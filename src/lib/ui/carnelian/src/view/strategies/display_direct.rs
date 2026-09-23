// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::app::strategies::framebuffer::{CoordinatorProxyPtr, DisplayId};
use crate::app::{Config, MessageInternal};
use crate::drawing::DisplayRotation;
use crate::render::generic::{self, Backend};
use crate::render::{Context as RenderContext, ContextInner};
use crate::view::strategies::base::{ViewStrategy, ViewStrategyPtr};
use crate::view::{
    DisplayInfo, UserInputMessage, ViewAssistantContext, ViewAssistantPtr, ViewDetails,
};
use crate::{IntPoint, IntSize, Size, ViewKey, input};
use anyhow::{Context, Error, bail, ensure};
use async_trait::async_trait;
use display_utils::{
    BufferCollectionId, EventId, INVALID_LAYER_ID, ImageId as DisplayImageId, LayerId, PixelFormat,
};
use euclid::size2;
use fidl_fuchsia_hardware_display::{
    ConfigStamp, CoordinatorCommitConfigRequest, CoordinatorListenerRequest, CoordinatorProxy,
    INVALID_CONFIG_STAMP_VALUE,
};
use fidl_fuchsia_hardware_display_types::{INVALID_DISP_ID, ImageBufferUsage, ImageMetadata};
use fuchsia_async::{self as fasync};
use fuchsia_framebuffer::sysmem::BufferCollectionAllocator;
use fuchsia_framebuffer::{FrameSet, FrameUsage, ImageId};
use fuchsia_trace::{duration, instant};
use futures::channel::mpsc::UnboundedSender;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use zx::{Event, MonotonicDuration, MonotonicInstant, Status};

type WaitEvents = BTreeMap<ImageId, (Event, EventId)>;

struct BusyImage {
    stamp: ConfigStamp,
    view_key: ViewKey,
    image_id: DisplayImageId,
    collection_id: BufferCollectionId,
}

#[derive(Default)]
struct CollectionIdGenerator {}

impl Iterator for CollectionIdGenerator {
    type Item = BufferCollectionId;

    fn next(&mut self) -> Option<BufferCollectionId> {
        static NEXT_ID_VALUE: AtomicU64 = AtomicU64::new(100);
        // NEXT_ID_VALUE only increments so it only requires atomicity, and we
        // can use Relaxed order.
        let value = NEXT_ID_VALUE.fetch_add(1, Ordering::Relaxed);
        // fetch_add wraps on overflow, which we'll use as a signal
        // that this generator is out of ids.
        if value == 0 { None } else { Some(BufferCollectionId(value)) }
    }
}
fn next_collection_id() -> BufferCollectionId {
    CollectionIdGenerator::default().next().expect("collection_id")
}

#[derive(Default)]
struct ImageIdGenerator {}

impl Iterator for ImageIdGenerator {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        // NEXT_ID only increments so it only requires atomicity, and we can
        // use Relaxed order.
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        // fetch_add wraps on overflow, which we'll use as a signal
        // that this generator is out of ids.
        if id == 0 { None } else { Some(id) }
    }
}
fn next_image_id() -> u64 {
    ImageIdGenerator::default().next().expect("image_id")
}

#[derive(Default)]
struct LayerIdGenerator {}

impl Iterator for LayerIdGenerator {
    type Item = LayerId;

    fn next(&mut self) -> Option<LayerId> {
        static NEXT_ID_VALUE: AtomicU64 = AtomicU64::new(1);
        // NEXT_ID_VALUE only increments so it only requires atomicity, and we
        // can use Relaxed order.
        let value = NEXT_ID_VALUE.fetch_add(1, Ordering::Relaxed);
        // fetch_add wraps on overflow, which we'll use as a signal
        // that this generator is out of ids.
        if value == 0 { None } else { Some(LayerId(value)) }
    }
}
fn next_layer_id() -> LayerId {
    LayerIdGenerator::default().next().expect("layer_id")
}

async fn create_and_import_event(
    coordinator: &CoordinatorProxy,
) -> Result<(Event, EventId), Error> {
    let event = Event::create();

    let their_event = event.duplicate_handle(zx::Rights::SAME_RIGHTS)?;
    let event_id_value = event.koid()?.raw_koid();
    let event_id = EventId(event_id_value);
    coordinator.import_event(their_event, &event_id.into())?;
    Ok((event, event_id))
}

fn size_from_info(info: &fidl_fuchsia_hardware_display::Info, mode_idx: usize) -> IntSize {
    let mode = &info.modes[mode_idx];
    size2(mode.active_area.width, mode.active_area.height).to_i32()
}

#[derive(Debug, Clone)]
pub struct Display {
    pub coordinator: CoordinatorProxyPtr,
    pub display_id: DisplayId,
    pub info: fidl_fuchsia_hardware_display::Info,
    pub layer_id: LayerId,
    pub mode_idx: usize,
}

impl Display {
    pub async fn new(
        coordinator: CoordinatorProxyPtr,
        display_id: DisplayId,
        info: fidl_fuchsia_hardware_display::Info,
    ) -> Result<Self, Error> {
        Ok(Self { coordinator, display_id, info, layer_id: INVALID_LAYER_ID, mode_idx: 0 })
    }

    pub fn set_mode(&mut self, mode_idx: usize) -> Result<(), Error> {
        self.coordinator.set_display_mode(&self.info.id, &self.info.modes[mode_idx])?;
        self.mode_idx = mode_idx;
        Ok(())
    }

    pub async fn create_layer(&mut self) -> Result<(), Error> {
        let layer_id = next_layer_id();
        let result = self.coordinator.create_layer(&layer_id.into()).await?;
        match result {
            Ok(()) => {
                self.layer_id = layer_id.into();
                Ok(())
            }
            Err(status) => {
                bail!("Display::new(): failed to create layer {}", Status::err_from_raw(status))
            }
        }
    }

    pub fn size(&self) -> IntSize {
        size_from_info(&self.info, self.mode_idx)
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.info.pixel_format[0].into()
    }
}

struct DisplayResources {
    pub frame_set: FrameSet,
    pub image_indexes: BTreeMap<ImageId, u32>,
    pub context: RenderContext,
    pub wait_events: WaitEvents,
    pub busy_images: VecDeque<BusyImage>,
}

const RENDER_FRAME_COUNT: usize = 2;

pub(crate) struct DisplayDirectViewStrategy {
    key: ViewKey,
    display: Display,
    app_sender: UnboundedSender<MessageInternal>,
    display_rotation: DisplayRotation,
    display_resources: Option<DisplayResources>,
    drop_display_resources_task: Option<fasync::Task<()>>,
    display_resource_release_delay: std::time::Duration,
    vsync_phase: MonotonicInstant,
    vsync_interval: MonotonicDuration,
    mouse_cursor_position: Option<IntPoint>,
    pub collection_id: BufferCollectionId,
    render_frame_count: usize,
    last_config_stamp: u64,

    /// ID of the framebuffer that has been prepared for presentation.
    prepared: Option<ImageId>,

    /// ID of the framebuffer submitted in the most recent present call.
    presented: Option<ImageId>,
}

impl DisplayDirectViewStrategy {
    pub async fn new(
        key: ViewKey,
        coordinator: CoordinatorProxyPtr,
        app_sender: UnboundedSender<MessageInternal>,
        info: fidl_fuchsia_hardware_display::Info,
        preferred_size: IntSize,
    ) -> Result<ViewStrategyPtr, Error> {
        let app_config = Config::get();
        let collection_id = next_collection_id();
        let render_frame_count = app_config.buffer_count.unwrap_or(RENDER_FRAME_COUNT);

        // Find first mode with the preferred size. Use preferred mode if not found.
        let mode_idx = info
            .modes
            .iter()
            .position(|mode| {
                let size = size2(mode.active_area.width, mode.active_area.height).to_i32();
                size == preferred_size
            })
            .unwrap_or(0);

        let mut display = Display::new(coordinator, info.id.into(), info).await?;

        if mode_idx != 0 {
            display.set_mode(mode_idx)?;
        }
        display.create_layer().await?;

        let display_resources = Self::allocate_display_resources(
            collection_id,
            display.size(),
            display.pixel_format(),
            render_frame_count,
            &display,
        )
        .await?;

        app_sender.unbounded_send(MessageInternal::Render(key)).expect("unbounded_send");
        app_sender.unbounded_send(MessageInternal::Focus(key, true)).expect("unbounded_send");

        Ok(Box::new(Self {
            key,
            display,
            app_sender,
            display_rotation: app_config.display_rotation,
            display_resources: Some(display_resources),
            drop_display_resources_task: None,
            display_resource_release_delay: app_config.display_resource_release_delay,
            vsync_phase: MonotonicInstant::get(),
            vsync_interval: MonotonicDuration::from_millis(16),
            mouse_cursor_position: None,
            collection_id,
            render_frame_count,
            last_config_stamp: INVALID_CONFIG_STAMP_VALUE,
            prepared: None,
            presented: None,
        }))
    }

    fn make_context(
        &mut self,
        view_details: &ViewDetails,
        image_id: Option<ImageId>,
    ) -> ViewAssistantContext {
        let time_now = MonotonicInstant::get();
        // |interval_offset| is the offset from |time_now| to the next multiple
        // of vsync interval after vsync phase, possibly negative if in the past.
        let mut interval_offset = MonotonicDuration::from_nanos(
            (self.vsync_phase.into_nanos() - time_now.into_nanos())
                % self.vsync_interval.into_nanos(),
        );
        // Unless |time_now| is exactly on the interval, adjust forward to the next
        // vsync after |time_now|.
        if interval_offset != MonotonicDuration::from_nanos(0) && self.vsync_phase < time_now {
            interval_offset += self.vsync_interval;
        }

        let display_rotation = self.display_rotation;
        let app_sender = self.app_sender.clone();
        let mouse_cursor_position = self.mouse_cursor_position.clone();
        let (image_index, actual_image_id) = image_id
            .and_then(|available| {
                Some((
                    *self.display_resources().image_indexes.get(&available).expect("image_index"),
                    available,
                ))
            })
            .unwrap_or_default();

        ViewAssistantContext {
            key: view_details.key,
            size: match display_rotation {
                DisplayRotation::Deg0 | DisplayRotation::Deg180 => view_details.physical_size,
                DisplayRotation::Deg90 | DisplayRotation::Deg270 => {
                    size2(view_details.physical_size.height, view_details.physical_size.width)
                }
            },
            metrics: view_details.metrics,
            presentation_time: time_now + interval_offset,
            buffer_count: None,
            image_id: actual_image_id,
            image_index: image_index,
            app_sender,
            mouse_cursor_position,
            display_info: Some(DisplayInfo::from(&self.display.info)),
        }
    }

    async fn allocate_display_resources(
        collection_id: BufferCollectionId,
        size: IntSize,
        pixel_format: display_utils::PixelFormat,
        render_frame_count: usize,
        display: &Display,
    ) -> Result<DisplayResources, Error> {
        let app_config = Config::get();
        let use_spinel = app_config.use_spinel;

        ensure!(use_spinel == false, "Spinel support is disabled");

        let display_rotation = app_config.display_rotation;
        let unsize = size.floor().to_u32();

        let usage = if use_spinel { FrameUsage::Gpu } else { FrameUsage::Cpu };
        let mut buffer_allocator = BufferCollectionAllocator::new(
            unsize.width,
            unsize.height,
            pixel_format.into(),
            usage,
            render_frame_count,
        )?;

        buffer_allocator.set_name(100, "CarnelianDirect")?;

        let context_token = buffer_allocator.duplicate_token().await?;
        let context = RenderContext {
            inner: ContextInner::Forma(generic::Forma::new_context(
                context_token,
                unsize,
                display_rotation,
            )),
        };

        let coordinator_token = buffer_allocator.duplicate_token().await?;
        // Sysmem token channels serve both sysmem(1) and sysmem2, so we can convert here until
        // display has an import_buffer_collection that takes a sysmem2 token.
        display
            .coordinator
            .import_buffer_collection(&collection_id.into(), coordinator_token)
            .await?
            .map_err(zx::Status::err_from_raw)?;
        display
            .coordinator
            .set_buffer_collection_constraints(
                &collection_id.into(),
                &ImageBufferUsage {
                    tiling_type: fidl_fuchsia_hardware_display_types::IMAGE_TILING_TYPE_LINEAR,
                },
            )
            .await?
            .map_err(zx::Status::err_from_raw)?;

        let buffers = buffer_allocator
            .allocate_buffers(true)
            .await
            .context(format!("view: {:?} allocate_buffers", display.display_id))?;

        ensure!(
            buffers.settings.as_ref().unwrap().image_format_constraints.is_some(),
            "No image format constraints"
        );
        ensure!(
            buffers
                .settings
                .as_ref()
                .unwrap()
                .image_format_constraints
                .as_ref()
                .unwrap()
                .pixel_format_modifier
                .is_some(),
            "Sysmem will always set pixel_format_modifier"
        );
        ensure!(
            buffers.buffers.as_ref().unwrap().len() == render_frame_count,
            "Buffers do not match frame count"
        );

        let image_tiling_type = match buffers
            .settings
            .as_ref()
            .unwrap()
            .image_format_constraints
            .as_ref()
            .unwrap()
            .pixel_format_modifier
            .as_ref()
            .unwrap()
        {
            fidl_fuchsia_images2::PixelFormatModifier::IntelI915XTiled => 1,
            fidl_fuchsia_images2::PixelFormatModifier::IntelI915YTiled => 2,
            _ => fidl_fuchsia_hardware_display_types::IMAGE_TILING_TYPE_LINEAR,
        };

        let image_metadata = ImageMetadata {
            dimensions: fidl_fuchsia_math::SizeU { width: unsize.width, height: unsize.height },
            tiling_type: image_tiling_type,
        };

        let mut image_ids = BTreeSet::new();
        let mut image_indexes = BTreeMap::new();
        let mut wait_events = WaitEvents::new();
        let buffer_count = buffers.buffers.as_ref().unwrap().len();
        for index in 0..buffer_count as usize {
            let uindex = index as u32;
            let image_id = next_image_id();
            let display_image_id = DisplayImageId(image_id);
            display
                .coordinator
                .import_image(
                    &image_metadata,
                    &collection_id.into(),
                    uindex,
                    &display_image_id.into(),
                )
                .await
                .context("FIDL coordinator import_image")?
                .map_err(zx::Status::err_from_raw)
                .context("import image error")?;

            image_ids.insert(image_id as u64);
            image_indexes.insert(image_id as u64, uindex);

            let (event, event_id) = create_and_import_event(&display.coordinator).await?;
            wait_events.insert(image_id as ImageId, (event, event_id));
        }

        let frame_set = FrameSet::new(image_ids);

        display.coordinator.set_layer_primary_config(&display.layer_id.into(), &image_metadata)?;

        Ok(DisplayResources {
            frame_set,
            image_indexes,
            context,
            wait_events,
            busy_images: VecDeque::new(),
        })
    }

    async fn maybe_reallocate_display_resources(&mut self) -> Result<(), Error> {
        if self.display_resources.is_none() {
            instant!(
                c"gfx",
                c"DisplayDirectViewStrategy::allocate_display_resources",
                fuchsia_trace::Scope::Process,
                "" => ""
            );
            self.collection_id = next_collection_id();
            self.prepared = None;
            self.presented = None;
            self.display_resources = Some(
                Self::allocate_display_resources(
                    self.collection_id,
                    self.display.size(),
                    self.display.pixel_format(),
                    self.render_frame_count,
                    &self.display,
                )
                .await?,
            );
        }
        Ok(())
    }

    fn display_resources(&mut self) -> &mut DisplayResources {
        self.display_resources.as_mut().expect("display_resources")
    }

    fn update_image(
        &mut self,
        view_details: &ViewDetails,
        view_assistant: &mut ViewAssistantPtr,
        image: u64,
    ) {
        instant!(
            c"gfx",
            c"DisplayDirectViewStrategy::update_image",
            fuchsia_trace::Scope::Process,
            "image" => format!("{}", image).as_str()
        );
        let (event, _) = self.display_resources().wait_events.get(&image).expect("wait event");
        let buffer_ready_event =
            event.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicate_handle");
        let direct_context = self.make_context(view_details, Some(image));

        view_assistant
            .render(&mut self.display_resources().context, buffer_ready_event, &direct_context)
            .unwrap_or_else(|e| panic!("Update error: {:?}", e));
    }

    fn handle_vsync_parameters_changed(
        &mut self,
        phase: MonotonicInstant,
        interval: MonotonicDuration,
    ) {
        self.vsync_phase = phase;
        self.vsync_interval = interval;
    }
}

#[async_trait(?Send)]
impl ViewStrategy for DisplayDirectViewStrategy {
    fn initial_metrics(&self) -> Size {
        size2(1.0, 1.0)
    }

    fn initial_physical_size(&self) -> Size {
        self.display.size().to_f32()
    }

    fn initial_logical_size(&self) -> Size {
        self.display.size().to_f32()
    }

    fn create_view_assistant_context(&self, view_details: &ViewDetails) -> ViewAssistantContext {
        ViewAssistantContext {
            key: view_details.key,
            size: match self.display_rotation {
                DisplayRotation::Deg0 | DisplayRotation::Deg180 => view_details.physical_size,
                DisplayRotation::Deg90 | DisplayRotation::Deg270 => {
                    size2(view_details.physical_size.height, view_details.physical_size.width)
                }
            },
            metrics: view_details.metrics,
            presentation_time: Default::default(),
            buffer_count: None,
            image_id: Default::default(),
            image_index: Default::default(),
            app_sender: self.app_sender.clone(),
            mouse_cursor_position: self.mouse_cursor_position.clone(),
            display_info: Some(DisplayInfo::from(&self.display.info)),
        }
    }

    fn setup(&mut self, view_details: &ViewDetails, view_assistant: &mut ViewAssistantPtr) {
        if let Some(available) = self.display_resources().frame_set.take_image() {
            let direct_context = self.make_context(view_details, Some(available));
            view_assistant
                .setup(&direct_context)
                .unwrap_or_else(|e| panic!("Setup error: {:?}", e));
            self.display_resources().frame_set.return_image(available);
        }
    }

    async fn render(
        &mut self,
        view_details: &ViewDetails,
        view_assistant: &mut ViewAssistantPtr,
    ) -> bool {
        duration!("gfx", "DisplayDirectViewStrategy::update");
        self.maybe_reallocate_display_resources()
            .await
            .expect("maybe_reallocate_display_resources");

        let prepared_image: Option<ImageId> =
            self.display_resources().frame_set.take_image().or_else(|| {
                // If there is only one render framebuffer, we always use that
                // framebuffer, even if it's being presented.
                if self.render_frame_count == 1 { self.presented } else { None }
            });

        if let Some(prepared_image) = prepared_image {
            self.update_image(view_details, view_assistant, prepared_image);
            self.prepared = Some(prepared_image);
            true
        } else {
            false
        }
    }

    fn present(&mut self, view_details: &ViewDetails) {
        duration!("gfx", "DisplayDirectViewStrategy::present");

        let image_to_present = self.prepared.take();
        if let Some(image_to_present) = image_to_present {
            instant!(
                c"gfx",
                c"DisplayDirectViewStrategy::present",
                fuchsia_trace::Scope::Process,
                "image_to_present" => format!("{}", image_to_present).as_str()
            );
            let collection_id = self.collection_id;
            let view_key = view_details.key;

            let layers = &[self.display.layer_id.into()];
            if layers.len() as u32 > self.display.info.max_layer_count {
                panic!(
                    "Hardware layer limit ({}) is insufficient for Carnelian (requested {})",
                    self.display.info.max_layer_count,
                    layers.len()
                );
            }

            self.display
                .coordinator
                .set_display_layers(&self.display.display_id.into(), layers)
                .expect("set_display_layers");

            let (_, wait_event_id) =
                *self.display_resources().wait_events.get(&image_to_present).expect("wait event");

            let image_id = DisplayImageId(image_to_present);
            self.display
                .coordinator
                .set_layer_image2(
                    &self.display.layer_id.into(),
                    &image_id.into(),
                    &wait_event_id.into(),
                )
                .expect("Frame::present() set_layer_image2");

            self.last_config_stamp += 1;
            let stamp = ConfigStamp { value: self.last_config_stamp };
            let req = CoordinatorCommitConfigRequest { stamp: Some(stamp), ..Default::default() };

            self.display.coordinator.commit_config(req).expect("Frame::present() commit_config");

            if self.render_frame_count > 1 {
                self.display_resources().busy_images.push_back(BusyImage {
                    stamp,
                    view_key,
                    image_id,
                    collection_id,
                });
            }
            self.presented = Some(image_to_present);
        }
    }

    fn handle_focus(
        &mut self,
        view_details: &ViewDetails,
        view_assistant: &mut ViewAssistantPtr,
        focus: bool,
    ) {
        let mut direct_context = self.make_context(view_details, None);
        view_assistant
            .handle_focus_event(&mut direct_context, focus)
            .unwrap_or_else(|e| panic!("handle_focus error: {:?}", e));
    }

    fn convert_user_input_message(
        &mut self,
        _view_details: &ViewDetails,
        _message: UserInputMessage,
    ) -> Result<Vec<crate::input::Event>, Error> {
        bail!("convert_user_input_message not used for display_direct.")
    }

    fn inspect_event(&mut self, view_details: &ViewDetails, event: &crate::input::Event) {
        match &event.event_type {
            input::EventType::Mouse(mouse_event) => {
                self.mouse_cursor_position = Some(mouse_event.location);
                self.app_sender
                    .unbounded_send(MessageInternal::RequestRender(view_details.key))
                    .expect("unbounded_send");
            }
            _ => (),
        };
    }

    fn image_freed(&mut self, image_id: u64, collection_id: u32) {
        if BufferCollectionId(collection_id as u64) == self.collection_id {
            instant!(
                c"gfx",
                c"DisplayDirectViewStrategy::image_freed",
                fuchsia_trace::Scope::Process,
                "image_freed" => format!("{}", image_id).as_str()
            );
            if let Some(display_resources) = self.display_resources.as_mut() {
                display_resources.frame_set.return_image(image_id);
            }
        }
    }

    fn ownership_changed(&mut self, owned: bool) {
        if !owned {
            let timer = fasync::Timer::new(fuchsia_async::MonotonicInstant::after(
                self.display_resource_release_delay.into(),
            ));
            let timer_sender = self.app_sender.clone();
            let task = fasync::Task::local(async move {
                timer.await;
                timer_sender
                    .unbounded_send(MessageInternal::DropDisplayResources)
                    .expect("unbounded_send");
            });
            self.drop_display_resources_task = Some(task);
        } else {
            self.drop_display_resources_task = None;
        }
    }

    fn drop_display_resources(&mut self) {
        let task = self.drop_display_resources_task.take();
        if task.is_some() {
            instant!(
                c"gfx",
                c"DisplayDirectViewStrategy::drop_display_resources",
                fuchsia_trace::Scope::Process,
                "" => ""
            );
            self.prepared = None;
            self.presented = None;
            self.display_resources = None;
        }
    }

    async fn handle_display_coordinator_listener_request(
        &mut self,
        event: CoordinatorListenerRequest,
    ) {
        match event {
            CoordinatorListenerRequest::OnVsync {
                timestamp,
                cookie,
                displayed_config_stamp,
                ..
            } => {
                duration!("gfx", "DisplayDirectViewStrategy::OnVsync");
                let vsync_interval = MonotonicDuration::from_nanos(
                    1_000_000_000_000 / self.display.info.modes[0].refresh_rate_millihertz as i64,
                );
                self.handle_vsync_parameters_changed(timestamp, vsync_interval);
                if cookie.value != INVALID_DISP_ID {
                    self.display
                        .coordinator
                        .acknowledge_vsync(cookie.value)
                        .expect("acknowledge_vsync");
                }

                let signal_sender = self.app_sender.clone();

                // Busy images are stamped with monotonically increasing values (because the last
                // stamp added to the deque is always greater than the previous).  So when we see a
                // vsync stamp, all images with a *strictly-lesser* stamp are now available for
                // reuse (images with an *equal* stamp are the ones currently displayed on-screen,
                // so can't be reused yet).
                if let Some(display_resources) = self.display_resources.as_mut() {
                    let busy_images = &mut display_resources.busy_images;
                    while !busy_images.is_empty() {
                        let front = &busy_images.front().unwrap();
                        if displayed_config_stamp.value <= front.stamp.value {
                            break;
                        }

                        signal_sender
                            .unbounded_send(MessageInternal::ImageFreed(
                                front.view_key,
                                front.image_id.0,
                                front.collection_id.0 as u32,
                            ))
                            .expect("unbounded_send");

                        busy_images.pop_front();
                    }
                }

                signal_sender
                    .unbounded_send(MessageInternal::Render(self.key))
                    .expect("unbounded_send");
            }
            CoordinatorListenerRequest::OnDisplaysChanged { .. } => {
                eprintln!("Carnelian ignoring CoordinatorListenerRequest::OnDisplaysChanged");
            }
            CoordinatorListenerRequest::OnClientOwnershipChange { has_ownership, .. } => {
                eprintln!(
                    "Carnelian ignoring CoordinatorListenerRequest::OnClientOwnershipChange (value: {})",
                    has_ownership
                );
            }
            _ => (),
        }
    }

    fn is_hosted_on_display(&self, display_id: DisplayId) -> bool {
        self.display.display_id == display_id
    }

    fn close(&mut self) {
        self.display
            .coordinator
            .release_buffer_collection(&self.collection_id.into())
            .expect("release_buffer_collection");
    }
}
