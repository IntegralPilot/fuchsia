// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::gpu_command::{GpuCommandSender, ScanoutController};
use crate::resource::Resource2D;
use crate::wire;
use anyhow::{anyhow, Context, Error};
use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints::{create_proxy, ClientEnd, ServerEnd};
use fidl_fuchsia_element::{GraphicalPresenterMarker, ViewSpec};
use fidl_fuchsia_ui_composition::{
    BlendMode, ColorRgba, ContentId, FlatlandEvent, FlatlandMarker, FlatlandProxy, ImageProperties,
    LayoutInfo, ParentViewportWatcherMarker, PresentArgs, TransformId, ViewBoundProtocols,
};
use fidl_fuchsia_ui_input3::KeyboardListenerMarker;
use fidl_fuchsia_ui_pointer::MouseSourceMarker;
use fidl_fuchsia_ui_views::{ViewCreationToken, ViewIdentityOnCreation, ViewportCreationToken};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_scenic::ViewRefPair;
use futures::channel::oneshot;
use futures::{select, StreamExt};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use {fidl_fuchsia_math as fmath, fuchsia_async as fasync};

const BACKGROUND_COLOR: ColorRgba = ColorRgba { red: 0.5, green: 0.5, blue: 0.5, alpha: 1.0 };

/// ContentId to use to detach content with a transform.
const NULL_CONTENT_ID: ContentId = ContentId { value: 0 };

fn new_transform_id(counter: &mut u64) -> TransformId {
    let value = *counter;
    *counter += 1;
    TransformId { value }
}

fn new_content_id(counter: &mut u64) -> ContentId {
    let value = *counter;
    *counter += 1;
    ContentId { value }
}

fn create_view_creation_tokens() -> (ViewCreationToken, ViewportCreationToken) {
    let (c1, c2) = zx::Channel::create();
    let view_token = ViewCreationToken { value: c1 };
    let viewport_token = ViewportCreationToken { value: c2 };
    (view_token, viewport_token)
}

pub trait Scanout {
    fn present(&mut self) -> Result<(), Error>;
    fn set_resource<'a>(
        &mut self,
        cmd: &wire::VirtioGpuSetScanout,
        resource: Option<&Resource2D<'a>>,
    ) -> Result<(), Error>;
    fn layout_info(&self) -> LayoutInfo;
    fn set_layout_info(&mut self, layout_info: LayoutInfo);
    fn update_cursor_resource<'a>(
        &mut self,
        resource: Option<&Resource2D<'a>>,
        cursor_hotspot: (u32, u32),
    ) -> Result<(), Error>;
    fn update_cursor_position_and_commit(
        &mut self,
        pos: wire::VirtioGpuCursorPos,
    ) -> Result<(), Error>;
}

pub struct FlatlandScanout {
    flatland: FlatlandProxy,
    next_flatland_id: u64,
    imported_resources: HashMap<u32, ContentId>,
    root_content_id: ContentId,
    content_transform_id: TransformId,
    present_credits: Rc<Cell<u64>>,
    layout_info: LayoutInfo,
    _event_handler: fasync::Task<()>,
    cursor_transform_id: TransformId,
}

impl Scanout for FlatlandScanout {
    fn present(&mut self) -> Result<(), Error> {
        let credits = self.present_credits.get();
        // TODO(https://fxbug.dev/42053853): For now we just skip the present if we don't have any credits.
        // We need to improve this so that once we do get a credit we'll eventually flush any
        // changes to screen.
        if credits > 0 {
            self.flatland.present(PresentArgs::default())?;
            self.present_credits.set(credits - 1);
        }
        Ok(())
    }

    fn set_resource<'a>(
        &mut self,
        _cmd: &wire::VirtioGpuSetScanout,
        resource: Option<&Resource2D<'a>>,
    ) -> Result<(), Error> {
        if let Some(resource) = resource {
            // TODO(https://fxbug.dev/42053853): We need to set the source region from the rect in the
            // VirtioGpuSetScanout command.
            //
            // In practice for single scanout configurations drivers usually provide precisely
            // sized resources.
            let content_id = self.get_or_create_resource(resource)?;
            self.flatland.set_content(&self.content_transform_id, &content_id)?;
        } else {
            self.flatland.set_content(&self.content_transform_id, &NULL_CONTENT_ID)?;
        }
        Ok(())
    }

    fn layout_info(&self) -> LayoutInfo {
        self.layout_info.clone()
    }

    fn set_layout_info(&mut self, layout_info: LayoutInfo) {
        if layout_info.logical_size.is_none() {
            log::error!("Received LayoutInfo without logical size");
            return;
        }
        self.layout_info = layout_info;

        // Update our background node
        let result = (|| -> Result<(), Error> {
            self.flatland.set_solid_fill(
                &self.root_content_id,
                &BACKGROUND_COLOR,
                &self.layout_info.logical_size.unwrap(),
            )?;
            self.present()?;
            Ok(())
        })();
        if let Err(e) = result {
            log::warn!("Failed to set view layout: {}", e);
        }
    }

    // Note: Currently there is no way to hide or modify the mouse cursor drawn
    // by scene_manager for a particular view, so we just ignore the problem
    // and draw the cursor as specified by the guest into our view.
    fn update_cursor_resource<'a>(
        &mut self,
        resource: Option<&Resource2D<'a>>,
        _cursor_hotspot: (u32, u32),
    ) -> Result<(), Error> {
        let content_id = if let Some(resource) = resource {
            let content_id = self.get_or_create_resource(resource)?;
            self.flatland.set_image_blending_function(&content_id, BlendMode::SrcOver)?;
            content_id
        } else {
            NULL_CONTENT_ID.clone()
        };

        self.flatland.set_content(&self.cursor_transform_id, &content_id)?;

        Ok(())
    }

    fn update_cursor_position_and_commit(
        &mut self,
        pos: wire::VirtioGpuCursorPos,
    ) -> Result<(), Error> {
        // pos.x and pos.y are signed integers representing the position of the top-left corner of
        // the cursor resource.
        let x = pos.x.get() as i32;
        let y = pos.y.get() as i32;
        self.flatland.set_translation(&self.cursor_transform_id, &fmath::Vec_ { x, y })?;
        self.present()?;
        Ok(())
    }
}

impl FlatlandScanout {
    /// Creates a new `Scanout`.
    ///
    /// The creation returns once the backing view for the `Scanout` has been created and presented
    /// to the user.
    pub async fn attach(
        mut command_sender: GpuCommandSender,
        keyboard_listener: Option<ClientEnd<KeyboardListenerMarker>>,
        mouse_source: Option<ServerEnd<MouseSourceMarker>>,
    ) -> Result<(), Error> {
        let flatland =
            connect_to_protocol::<FlatlandMarker>().context("error connecting to Flatland")?;

        // Create a view.
        let (parent_viewport_watcher, parent_viewport_watcher_request) =
            create_proxy::<ParentViewportWatcherMarker>();
        let (view_token, viewport_token) = create_view_creation_tokens();
        let viewref_pair = ViewRefPair::new()?;
        let view_ref_for_presenter = fuchsia_scenic::duplicate_view_ref(&viewref_pair.view_ref)?;
        let view_ref_for_keyboard = fuchsia_scenic::duplicate_view_ref(&viewref_pair.view_ref)?;
        let view_identity = ViewIdentityOnCreation::from(viewref_pair);
        let view_bound_protocols = ViewBoundProtocols { mouse_source, ..Default::default() };
        flatland
            .create_view2(
                view_token,
                view_identity,
                view_bound_protocols,
                parent_viewport_watcher_request,
            )
            .context("Failed to create_view")?;

        // If we have a keyboard listener for a virtio-input device, connect it to our view now.
        let keyboard = if let Some(keyboard_listener) = keyboard_listener {
            let keyboard = connect_to_protocol::<fidl_fuchsia_ui_input3::KeyboardMarker>()?;
            match keyboard.add_listener(view_ref_for_keyboard, keyboard_listener).await {
                Err(e) => {
                    log::warn!("Failed to register keyboard listener: {}", e);
                    None
                }
                Ok(()) => Some(keyboard),
            }
        } else {
            None
        };

        // Display with graphical presenter.
        let view_spec = ViewSpec {
            viewport_creation_token: Some(viewport_token),
            view_ref: Some(view_ref_for_presenter),
            ..Default::default()
        };
        let graphical_presenter = connect_to_protocol::<GraphicalPresenterMarker>()
            .context("failed to connect to GraphicalPresenter service")?;
        graphical_presenter
            .present_view(view_spec, None, None)
            .await?
            .map_err(|e| anyhow!("{:?}", e))?;

        // Setup our root transform. This will be empty until we get a set_scanout with a non-zero
        // resource_id. We start at 1 because 0 is not a valid id.
        let mut next_flatland_id = 1u64;
        let root_transform_id = new_transform_id(&mut next_flatland_id);
        flatland.create_transform(&root_transform_id)?;

        // Create a root transform node that is just a solid color.
        let root_content_id = new_content_id(&mut next_flatland_id);
        flatland.create_filled_rect(&root_content_id)?;
        flatland.set_content(&root_transform_id, &root_content_id)?;
        flatland.set_root_transform(&root_transform_id)?;

        // Add a content node that will hold any attached resources.
        let content_transform_id = new_transform_id(&mut next_flatland_id);
        flatland.create_transform(&content_transform_id)?;
        flatland.add_child(&root_transform_id, &content_transform_id)?;

        // Add a cursor node on top of the scanout content.
        let cursor_transform_id = new_transform_id(&mut next_flatland_id);
        flatland.create_transform(&cursor_transform_id)?;
        flatland.add_child(&root_transform_id, &cursor_transform_id)?;

        // Wait for an initial layout. We don't bother attaching a scanout until we have an initial
        // size so that we can report the size of this scanout to the driver.
        let mut layout_stream =
            HangingGetStream::new_with_fn_ptr(parent_viewport_watcher, |p| p.get_layout());
        let initial_layout = loop {
            match layout_stream.next().await {
                Some(Ok(layout)) => {
                    if layout.logical_size.is_some() {
                        break layout;
                    }
                }
                Some(Err(e)) => {
                    return Err(anyhow!("Error received waiting for initial layout: {}", e));
                }
                None => {
                    return Err(anyhow!("View closed before initial layout received"));
                }
            }
        };

        // Perform an initial present.
        flatland.set_solid_fill(
            &root_content_id,
            &BACKGROUND_COLOR,
            &initial_layout.logical_size.unwrap(),
        )?;
        flatland.present(PresentArgs::default())?;

        // Read events from the event streams to track the present credit count. We can only call
        // present if our counter is non-zero.
        //
        // Note that we initialize to 0 here because we consumed the initial present credit
        // presenting the initial view.
        let credit_counter = Rc::new(Cell::new(0));
        let present_credits = credit_counter.clone();
        let mut flatland_event_stream = flatland.take_event_stream();

        // Spawn an async event handler task to manage different event streams associated with the
        // view for this scanout.
        //
        // The additional oneshot channel is needed to solve a bit of a circular dependency between
        // this task and the scanout being attached to the device. Once the scanout has been
        // attached to the device, the controller will be passed to this task over the channel.
        let (sender, receiver) = oneshot::channel::<ScanoutController>();
        let event_handler = fasync::Task::local(async move {
            // We need to keep the connection to the keyboard protocol open as long as we want to
            // continue to receive events.
            let _keyboard = keyboard;

            let mut scanout_controller = receiver.await.unwrap();

            loop {
                select! {
                    flatland_event = flatland_event_stream.next() => match &flatland_event {
                        Some(Ok(FlatlandEvent::OnNextFrameBegin { values })) => {
                            if let Some(credits) = values.additional_present_credits {
                                credit_counter.set(credit_counter.get() + credits as u64);
                            }
                        }
                        _ => {}
                    },
                    layout_event = layout_stream.next() => match layout_event {
                        Some(Ok(layout_info)) => {
                            scanout_controller.resize(layout_info).await.unwrap();
                        }
                        _ => break,
                    },
                }
            }
            scanout_controller.detach().await.unwrap();
        });

        let scanout = Box::new(Self {
            flatland,
            next_flatland_id,
            imported_resources: HashMap::new(),
            root_content_id,
            content_transform_id,
            present_credits,
            layout_info: initial_layout,
            _event_handler: event_handler,
            cursor_transform_id,
        });
        let scanout_controller = command_sender.attach_scanout(scanout).await?;
        sender.send(scanout_controller).map_err(|_| {}).unwrap();
        Ok(())
    }

    fn get_or_create_resource<'a>(
        &mut self,
        resource: &Resource2D<'a>,
    ) -> Result<ContentId, Error> {
        if let Some(content_id) = self.imported_resources.get(&resource.id()) {
            Ok(*content_id)
        } else {
            let content_id = self.import_resource(resource)?;
            self.imported_resources.insert(resource.id(), content_id);
            Ok(content_id)
        }
    }

    fn import_resource<'a>(&mut self, resource: &Resource2D<'a>) -> Result<ContentId, Error> {
        let content_id = new_content_id(&mut self.next_flatland_id);
        let image_props = ImageProperties {
            size: Some(fmath::SizeU { width: resource.width(), height: resource.height() }),
            ..Default::default()
        };

        let import_token = resource
            .import_token()
            .ok_or_else(|| anyhow!("No import_token available for resource VMO"))?;

        self.flatland
            .create_image(&content_id, import_token, 0, &image_props)
            .context("Failed to create image")?;
        let size = fmath::SizeU { width: resource.width(), height: resource.height() };
        self.flatland.set_image_destination_size(&content_id, &size)?;
        // Src blend mode will prevent any alpha blending with the background.
        self.flatland.set_image_blending_function(&content_id, BlendMode::Src)?;
        Ok(content_id)
    }
}

#[cfg(test)]
pub struct FakeScanout {
    attached_resource: Option<u32>,
    layout_info: LayoutInfo,
    present_count: u32,
}

#[cfg(test)]
impl FakeScanout {
    pub async fn attach(
        width: u32,
        height: u32,
        mut command_sender: GpuCommandSender,
    ) -> Result<ScanoutController, Error> {
        let layout_info =
            LayoutInfo { logical_size: Some(fmath::SizeU { width, height }), ..Default::default() };

        let scanout = Box::new(Self { attached_resource: None, present_count: 0, layout_info });
        Ok(command_sender.attach_scanout(scanout).await?)
    }
}

#[cfg(test)]
impl Scanout for FakeScanout {
    fn present(&mut self) -> Result<(), Error> {
        self.present_count += 1;
        Ok(())
    }

    fn set_resource<'a>(
        &mut self,
        _cmd: &wire::VirtioGpuSetScanout,
        resource: Option<&Resource2D<'a>>,
    ) -> Result<(), Error> {
        self.attached_resource = resource.map(|r| r.id());
        Ok(())
    }

    fn layout_info(&self) -> LayoutInfo {
        self.layout_info.clone()
    }

    fn set_layout_info(&mut self, layout_info: LayoutInfo) {
        self.layout_info = layout_info;
    }

    fn update_cursor_resource<'a>(
        &mut self,
        _resource: Option<&Resource2D<'a>>,
        _cursor_hotspot: (u32, u32),
    ) -> Result<(), Error> {
        // No way to inspect cursor or scanout state from the tests so just return success.
        Ok(())
    }
    fn update_cursor_position_and_commit(
        &mut self,
        _pos: wire::VirtioGpuCursorPos,
    ) -> Result<(), Error> {
        // No way to inspect cursor or scanout state from the tests so just return success.
        Ok(())
    }
}
