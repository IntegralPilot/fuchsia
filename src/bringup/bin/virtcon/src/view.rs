// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{MAX_FONT_SIZE, MIN_FONT_SIZE};
use crate::colors::ColorScheme;
use crate::terminal::Terminal;
use crate::text_grid::{TextGridFacet, TextGridMessages};
use anyhow::Error;
use carnelian::drawing::load_font;
use carnelian::render::Context as RenderContext;
use carnelian::scene::facets::FacetId;
use carnelian::scene::scene::{Scene, SceneBuilder, SceneOrder};
use carnelian::{
    AppSender, Point, Size, ViewAssistant, ViewAssistantContext, ViewAssistantPtr, ViewKey, input,
};
use fidl_fuchsia_hardware_power_statecontrol::{
    AdminMarker, AdminSynchronousProxy, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fidl_fuchsia_hardware_pty::WindowSize;
use fidl_fuchsia_input::ConsumerControlButton;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_channel_to_protocol;
use futures::future::{FutureExt as _, join_all};
use pty::key_util::{CodePoint, HidUsage};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::mem;
use std::path::PathBuf;
use terminal::renderer::Rgb;
use terminal::{FontSet, Scroll, SizeInfo, cell_size_from_cell_height, get_scale_factor};

fn is_control_only(modifiers: &input::Modifiers) -> bool {
    modifiers.control && !modifiers.shift && !modifiers.alt && !modifiers.caps_lock
}

fn get_input_sequence_for_key_event(
    event: &input::keyboard::Event,
    app_cursor: bool,
) -> Option<String> {
    match event.phase {
        input::keyboard::Phase::Pressed | input::keyboard::Phase::Repeat => {
            match event.code_point {
                None => HidUsage { hid_usage: event.hid_usage, app_cursor }.into(),
                Some(code_point) => CodePoint {
                    code_point: code_point,
                    control_pressed: is_control_only(&event.modifiers),
                }
                .into(),
            }
        }
        _ => None,
    }
}

pub enum ViewMessages {
    AddTerminalMessage(u32, Terminal, bool),
    RequestTerminalUpdateMessage(u32),
}

// Constraints on status bar tabs.
const MIN_TAB_WIDTH: usize = 16;
const MAX_TAB_WIDTH: usize = 32;

// Status bar colors.
const STATUS_COLOR_DEFAULT: Rgb = Rgb { r: 170, g: 170, b: 170 };
const STATUS_COLOR_ACTIVE: Rgb = Rgb { r: 255, g: 255, b: 85 };
const STATUS_COLOR_UPDATED: Rgb = Rgb { r: 85, g: 255, b: 85 };

// Amount of change to font size when zooming.
const FONT_SIZE_INCREMENT: f32 = 4.0;

// Maximum terminal size in cells. We support up to 4 layers per cell.
const MAX_CELLS: u32 = SceneOrder::MAX.as_u32() / 4;

struct SceneDetails {
    scene: Scene,
    textgrid: Option<FacetId>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TerminalStatus {
    pub has_output: bool,
    pub at_top: bool,
    pub at_bottom: bool,
}

// Handles and records repeated button presses.
//
// Repeated button presses are defined as one or more button presses where
// the interval between two consecutive events doesn't exceed 2 seconds.
struct RepeatedButtonPressHandler {
    // Durations of each button press for repeated button presses.
    //
    // `press_durations_ns` and `down_times_ns` are guaranteed to be of the
    // same size.
    press_durations_ns: Vec<u64>,

    // Timestamps of the press-down event for each button press for repeated
    // button presses.
    down_times_ns: Vec<u64>,

    button: ConsumerControlButton,

    // Timestamp of the most recent button down event waiting for
    // release.
    pending_down_time_ns: Option<u64>,
}

impl RepeatedButtonPressHandler {
    pub fn new(button: ConsumerControlButton) -> Self {
        Self {
            press_durations_ns: vec![],
            down_times_ns: vec![],
            button,
            pending_down_time_ns: None,
        }
    }

    // Returns true iff the event can be handled by this handler.
    pub fn handle_button_event(
        &mut self,
        event: &input::consumer_control::Event,
        time_ns: u64,
    ) -> bool {
        if event.button != self.button {
            return false;
        }

        if event.phase == carnelian::input::consumer_control::Phase::Down {
            self.pending_down_time_ns = Some(time_ns);
            return true;
        }

        // event.phase == carnelian::input::consumer_control::Phase::Up
        if let None = self.pending_down_time_ns {
            return false;
        }
        let down_time_ns: u64 =
            self.pending_down_time_ns.take().expect("pending press down time missing");
        let up_time_ns: u64 = time_ns;

        if up_time_ns < down_time_ns {
            return false;
        }
        let press_duration_ns: u64 = up_time_ns - down_time_ns;

        let last_down_time_ns: u64 = *self.down_times_ns.last().unwrap_or(&0);
        if down_time_ns < last_down_time_ns {
            return false;
        }

        const REPEATED_PRESS_THRESHOLD_NS: u64 =
            zx::MonotonicDuration::from_seconds(2).into_nanos() as u64;

        // down_time_ns is guaranteed to be >= last_down_time_ns.
        if down_time_ns - last_down_time_ns > REPEATED_PRESS_THRESHOLD_NS {
            self.press_durations_ns = vec![press_duration_ns];
            self.down_times_ns = vec![down_time_ns];
        } else {
            self.press_durations_ns.push(press_duration_ns);
            self.down_times_ns.push(down_time_ns);
        }

        true
    }
}

pub struct VirtualConsoleViewAssistant {
    app_sender: AppSender,
    view_key: ViewKey,
    color_scheme: ColorScheme,
    round_scene_corners: bool,
    font_size: f32,
    dpi: BTreeSet<u32>,
    cell_size: Size,
    tab_width: usize,
    scene_details: Option<SceneDetails>,
    terminals: BTreeMap<u32, (Terminal, TerminalStatus)>,
    font_set: FontSet,
    active_terminal_id: u32,
    owns_display: bool,
    active_pointer_id: Option<input::pointer::PointerId>,
    start_pointer_location: Point,
    power_button_press_handler: RepeatedButtonPressHandler,
}

const FONT: &'static str = "/pkg/data/font.ttf";
const BOLD_FONT_PATH_1: &'static str = "/pkg/data/bold-font.ttf";
const BOLD_FONT_PATH_2: &'static str = "/boot/data/bold-font.ttf";
const ITALIC_FONT_PATH_1: &'static str = "/pkg/data/italic-font.ttf";
const ITALIC_FONT_PATH_2: &'static str = "/boot/data/italic-font.ttf";
const BOLD_ITALIC_FONT_PATH_1: &'static str = "/pkg/data/bold-italic-font.ttf";
const BOLD_ITALIC_FONT_PATH_2: &'static str = "/boot/data/bold-italic-font.ttf";
const FALLBACK_FONT_PREFIX: &'static str = "/pkg/data/fallback-font";

impl VirtualConsoleViewAssistant {
    pub fn new(
        app_sender: &AppSender,
        view_key: ViewKey,
        color_scheme: ColorScheme,
        round_scene_corners: bool,
        font_size: f32,
        dpi: BTreeSet<u32>,
    ) -> Result<ViewAssistantPtr, Error> {
        let cell_size = Size::new(8.0, 16.0);
        let tab_width = MIN_TAB_WIDTH;
        let scene_details = None;
        let terminals = BTreeMap::new();
        let active_terminal_id = 0;
        let font = load_font(PathBuf::from(FONT))?;
        let bold_font = load_font(PathBuf::from(BOLD_FONT_PATH_1))
            .or_else(|_| load_font(PathBuf::from(BOLD_FONT_PATH_2)))
            .ok();
        let italic_font = load_font(PathBuf::from(ITALIC_FONT_PATH_1))
            .or_else(|_| load_font(PathBuf::from(ITALIC_FONT_PATH_2)))
            .ok();
        let bold_italic_font = load_font(PathBuf::from(BOLD_ITALIC_FONT_PATH_1))
            .or_else(|_| load_font(PathBuf::from(BOLD_ITALIC_FONT_PATH_2)))
            .ok();
        let mut fallback_fonts = vec![];
        while let Ok(font) = load_font(PathBuf::from(format!(
            "{}-{}.ttf",
            FALLBACK_FONT_PREFIX,
            fallback_fonts.len() + 1
        ))) {
            fallback_fonts.push(font);
        }
        let font_set = FontSet::new(font, bold_font, italic_font, bold_italic_font, fallback_fonts);
        let owns_display = true;
        let active_pointer_id = None;
        let start_pointer_location = Point::zero();
        let power_button_press_handler =
            RepeatedButtonPressHandler::new(ConsumerControlButton::Power);

        Ok(Box::new(VirtualConsoleViewAssistant {
            app_sender: app_sender.clone(),
            view_key,
            color_scheme,
            round_scene_corners,
            font_size,
            dpi,
            cell_size,
            tab_width,
            scene_details,
            terminals,
            font_set,
            active_terminal_id,
            owns_display,
            active_pointer_id,
            start_pointer_location,
            power_button_press_handler,
        }))
    }

    #[cfg(test)]
    fn new_for_test() -> Result<ViewAssistantPtr, Error> {
        let app_sender = AppSender::new_for_testing_purposes_only();
        let dpi: BTreeSet<u32> = [160, 320, 480, 640].iter().cloned().collect();
        Self::new(&app_sender, Default::default(), ColorScheme::default(), false, 14.0, dpi)
    }

    // Resize all terminals for 'new_size'.
    fn resize_terminals(&mut self, new_size: &Size, new_font_size: f32) {
        let cell_size = cell_size_from_cell_height(&self.font_set, new_font_size);
        let grid_size =
            Size::new(new_size.width / cell_size.width, new_size.height / cell_size.height).floor();
        // Clamp width to respect `MAX_CELLS`.
        let clamped_grid_size = if grid_size.area() > MAX_CELLS as f32 {
            assert!(
                grid_size.height <= MAX_CELLS as f32,
                "terminal height greater than MAX_CELLS: {}",
                grid_size.height
            );
            Size::new(MAX_CELLS as f32 / grid_size.height, grid_size.height).floor()
        } else {
            grid_size
        };
        let clamped_size = Size::new(
            clamped_grid_size.width * cell_size.width,
            clamped_grid_size.height * cell_size.height,
        );
        let size = Size::new(clamped_size.width, clamped_size.height - cell_size.height);
        let size_info = SizeInfo {
            width: size.width,
            height: size.height,
            cell_width: cell_size.width,
            cell_height: cell_size.height,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };

        self.cell_size = cell_size;

        for (terminal, _) in self.terminals.values_mut() {
            terminal.resize(&size_info);
        }

        // PTY window size (in character cells).
        let window_size = WindowSize {
            width: clamped_grid_size.width as u32,
            height: clamped_grid_size.height as u32 - 1,
        };

        let ptys: Vec<_> =
            self.terminals.values().filter_map(|(term, _)| term.pty()).cloned().collect();
        fasync::Task::local(async move {
            join_all(ptys.iter().map(|pty| {
                pty.resize(window_size).map(|result| result.expect("failed to set window size"))
            }))
            .map(|vec| vec.into_iter().collect())
            .await
        })
        .detach();
    }

    // This returns a vector with the status for each terminal. The return value
    // is suitable for passing to the TextGridFacet.
    fn get_status(&self) -> Vec<(String, Rgb)> {
        self.terminals
            .iter()
            .map(|(id, (t, status))| {
                let fg = if *id == self.active_terminal_id {
                    STATUS_COLOR_ACTIVE
                } else if status.has_output {
                    STATUS_COLOR_UPDATED
                } else {
                    STATUS_COLOR_DEFAULT
                };

                let left = if status.at_top { '[' } else { '<' };
                let right = if status.at_bottom { ']' } else { '>' };

                (format!("{}{}{} {}", left, *id, right, t.title()), fg)
            })
            .collect()
    }

    fn set_active_terminal(&mut self, id: u32) {
        if let Some((terminal, status)) = self.terminals.get_mut(&id) {
            self.active_terminal_id = id;
            status.has_output = false;
            let terminal = terminal.clone_term();
            let new_status = self.get_status();
            if let Some(scene_details) = &mut self.scene_details {
                if let Some(textgrid) = &scene_details.textgrid {
                    scene_details.scene.send_message(
                        textgrid,
                        Box::new(TextGridMessages::ChangeStatusMessage(new_status)),
                    );
                    scene_details.scene.send_message(
                        textgrid,
                        Box::new(TextGridMessages::SetTermMessage(terminal)),
                    );
                    self.app_sender.request_render(self.view_key);
                }
            }
        }
    }

    fn next_active_terminal(&mut self) {
        let first = self.terminals.keys().next();
        let last = self.terminals.keys().next_back();
        if let Some((first, last)) = first.and_then(|first| last.map(|last| (first, last))) {
            let active = self.active_terminal_id;
            let id = if active == *last { *first } else { active + 1 };
            self.set_active_terminal(id);
        }
    }

    fn previous_active_terminal(&mut self) {
        let first = self.terminals.keys().next();
        let last = self.terminals.keys().next_back();
        if let Some((first, last)) = first.and_then(|first| last.map(|last| (first, last))) {
            let active = self.active_terminal_id;
            let id = if active == *first { *last } else { active - 1 };
            self.set_active_terminal(id);
        }
    }

    fn update_status(&mut self) {
        let new_status = self.get_status();
        if let Some(scene_details) = &mut self.scene_details {
            if let Some(textgrid) = &scene_details.textgrid {
                scene_details.scene.send_message(
                    textgrid,
                    Box::new(TextGridMessages::ChangeStatusMessage(new_status)),
                );
                self.app_sender.request_render(self.view_key);
            }
        }
    }

    fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size;
        self.scene_details = None;
        self.app_sender.request_render(self.view_key);
    }

    fn scroll_active_terminal(&mut self, scroll: Scroll) {
        if let Some((terminal, _)) = self.terminals.get_mut(&self.active_terminal_id) {
            terminal.scroll(scroll);
        }
    }

    fn trigger_shutdown(&mut self, shutdown_action: ShutdownAction) -> Result<(), Error> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = (|| {
                let (server_end, client_end) = zx::Channel::create();
                connect_channel_to_protocol::<AdminMarker>(server_end)?;
                let admin = AdminSynchronousProxy::new(client_end);
                Ok(admin.shutdown(
                    &ShutdownOptions {
                        action: Some(shutdown_action),
                        reasons: Some(vec![ShutdownReason::DeveloperRequest]),
                        ..Default::default()
                    },
                    zx::MonotonicInstant::INFINITE,
                )?)
            })();
            let _ = tx.send(res);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(Ok(()))) => {
                println!("Shutdown call returned unexpectedly, sleeping forever.");
                zx::MonotonicInstant::INFINITE.sleep();
            }
            Ok(Ok(Err(e))) => println!("Failed to shutdown, status: {}", e),
            Ok(Err(e)) => return Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                println!("Failed to shutdown due to timeout.");
            }
            Err(_) => panic!("Background shutdown thread terminated unexpectedly."),
        }
        Ok(())
    }

    fn handle_device_control_consumer_control_event(
        &mut self,
        _context: &mut ViewAssistantContext,
        consumer_control_event: &input::consumer_control::Event,
        event_time_ns: u64,
    ) -> Result<bool, Error> {
        let power_button_press_handled = self
            .power_button_press_handler
            .handle_button_event(consumer_control_event, event_time_ns);
        if power_button_press_handled {
            const POWER_KEY_PRESS_COUNT_FOR_REBOOT: usize = 3;
            if self.power_button_press_handler.press_durations_ns.len()
                >= POWER_KEY_PRESS_COUNT_FOR_REBOOT
            {
                let last_press_duration =
                    *self.power_button_press_handler.press_durations_ns.last().expect("last");

                const REBOOT_TO_BOOTLOADER_THRESHOLD: u64 =
                    zx::MonotonicDuration::from_seconds(1).into_nanos() as u64;

                if last_press_duration > REBOOT_TO_BOOTLOADER_THRESHOLD {
                    self.trigger_shutdown(ShutdownAction::RebootToBootloader)?;
                } else {
                    self.trigger_shutdown(ShutdownAction::Reboot)?;
                }
            }
            return Ok(true);
        }

        Ok(false)
    }

    fn handle_device_control_keyboard_event(
        &mut self,
        _context: &mut ViewAssistantContext,
        keyboard_event: &input::keyboard::Event,
    ) -> Result<bool, Error> {
        if keyboard_event.phase == input::keyboard::Phase::Pressed {
            if keyboard_event.code_point.is_none() {
                const HID_USAGE_KEY_DELETE: u32 = 0x4c;

                let modifiers = &keyboard_event.modifiers;
                match keyboard_event.hid_usage {
                    // Provides a CTRL-ALT-DEL reboot sequence.
                    HID_USAGE_KEY_DELETE if modifiers.control && modifiers.alt => {
                        self.trigger_shutdown(ShutdownAction::Reboot)?;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }

        Ok(false)
    }

    fn handle_control_keyboard_event(
        &mut self,
        _context: &mut ViewAssistantContext,
        keyboard_event: &input::keyboard::Event,
    ) -> Result<bool, Error> {
        match keyboard_event.phase {
            input::keyboard::Phase::Pressed | input::keyboard::Phase::Repeat => {
                let modifiers = &keyboard_event.modifiers;
                match keyboard_event.code_point {
                    None => {
                        const HID_USAGE_KEY_TAB: u32 = 0x2b;
                        const HID_USAGE_KEY_F1: u32 = 0x3a;
                        const HID_USAGE_KEY_F10: u32 = 0x43;
                        const HID_USAGE_KEY_HOME: u32 = 0x4a;
                        const HID_USAGE_KEY_PAGEUP: u32 = 0x4b;
                        const HID_USAGE_KEY_END: u32 = 0x4d;
                        const HID_USAGE_KEY_PAGEDOWN: u32 = 0x4e;
                        const HID_USAGE_KEY_DOWN: u32 = 0x51;
                        const HID_USAGE_KEY_UP: u32 = 0x52;
                        const HID_USAGE_KEY_VOL_DOWN: u32 = 0xe8;
                        const HID_USAGE_KEY_VOL_UP: u32 = 0xe9;

                        match keyboard_event.hid_usage {
                            HID_USAGE_KEY_F1..=HID_USAGE_KEY_F10 if modifiers.alt => {
                                let id = keyboard_event.hid_usage - HID_USAGE_KEY_F1;
                                self.set_active_terminal(id);
                                return Ok(true);
                            }
                            HID_USAGE_KEY_TAB if modifiers.alt => {
                                if modifiers.shift {
                                    self.previous_active_terminal();
                                } else {
                                    self.next_active_terminal();
                                }
                                return Ok(true);
                            }
                            HID_USAGE_KEY_VOL_UP if modifiers.alt => {
                                self.previous_active_terminal();
                                return Ok(true);
                            }
                            HID_USAGE_KEY_VOL_DOWN if modifiers.alt => {
                                self.next_active_terminal();
                                return Ok(true);
                            }
                            HID_USAGE_KEY_UP if modifiers.alt => {
                                self.scroll_active_terminal(Scroll::Lines(1));
                                return Ok(true);
                            }
                            HID_USAGE_KEY_DOWN if modifiers.alt => {
                                self.scroll_active_terminal(Scroll::Lines(-1));
                                return Ok(true);
                            }
                            HID_USAGE_KEY_PAGEUP if modifiers.shift => {
                                self.scroll_active_terminal(Scroll::PageUp);
                                return Ok(true);
                            }
                            HID_USAGE_KEY_PAGEDOWN if modifiers.shift => {
                                self.scroll_active_terminal(Scroll::PageDown);
                                return Ok(true);
                            }
                            HID_USAGE_KEY_HOME if modifiers.shift => {
                                self.scroll_active_terminal(Scroll::Top);
                                return Ok(true);
                            }
                            HID_USAGE_KEY_END if modifiers.shift => {
                                self.scroll_active_terminal(Scroll::Bottom);
                                return Ok(true);
                            }
                            _ => {}
                        }
                    }
                    Some(code_point) if modifiers.alt == true => {
                        const PLUS: u32 = 43;
                        const EQUAL: u32 = 61;
                        const MINUS: u32 = 45;

                        match code_point {
                            PLUS | EQUAL => {
                                let new_font_size =
                                    (self.font_size + FONT_SIZE_INCREMENT).min(MAX_FONT_SIZE);
                                self.set_font_size(new_font_size);
                                return Ok(true);
                            }
                            MINUS => {
                                let new_font_size =
                                    (self.font_size - FONT_SIZE_INCREMENT).max(MIN_FONT_SIZE);
                                self.set_font_size(new_font_size);
                                return Ok(true);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        Ok(false)
    }
}

impl ViewAssistant for VirtualConsoleViewAssistant {
    fn resize(&mut self, _new_size: &Size) -> Result<(), Error> {
        self.scene_details = None;
        Ok(())
    }

    fn render(
        &mut self,
        render_context: &mut RenderContext,
        ready_event: zx::Event,
        context: &ViewAssistantContext,
    ) -> Result<(), Error> {
        let mut scene_details = self.scene_details.take().unwrap_or_else(|| {
            let mut builder = SceneBuilder::new()
                .background_color(self.color_scheme.back)
                .enable_mouse_cursor(false)
                .round_scene_corners(self.round_scene_corners)
                .mutable(false);

            let textgrid = {
                let scale_factor = if let Some(info) = context.display_info.as_ref() {
                    // Use 1.0 scale factor when fallback sizes are used as opposed
                    // to actual values reported by the display.
                    if info.using_fallback_size {
                        1.0
                    } else {
                        const MM_PER_INCH: f32 = 25.4;

                        let dpi = context.size.height * MM_PER_INCH / info.vertical_size_mm as f32;

                        get_scale_factor(&self.dpi, dpi)
                    }
                } else {
                    1.0
                };
                let cell_height = self.font_size * scale_factor;

                self.resize_terminals(&context.size, cell_height);

                let active_term =
                    self.terminals.get(&self.active_terminal_id).map(|(t, _)| t.clone_term());
                let status = self.get_status();
                let columns = active_term
                    .as_ref()
                    .map(|t| t.borrow().screen().size().1 as usize)
                    .unwrap_or(1);

                // Determine the status bar tab width based on the current number
                // of terminals.
                let tab_width =
                    (columns as usize / (status.len() + 1)).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH);

                let cell_size = cell_size_from_cell_height(&self.font_set, cell_height);

                // Add the text grid to the scene.
                let textgrid = builder.facet(Box::new(TextGridFacet::new(
                    self.font_set.clone(),
                    &cell_size,
                    self.color_scheme,
                    active_term,
                    status,
                    tab_width,
                )));

                self.cell_size = cell_size;
                self.tab_width = tab_width;

                Some(textgrid)
            };

            SceneDetails { scene: builder.build(), textgrid }
        });

        scene_details.scene.render(render_context, ready_event, context)?;
        self.scene_details = Some(scene_details);

        Ok(())
    }

    fn handle_consumer_control_event(
        &mut self,
        context: &mut ViewAssistantContext,
        event: &input::Event,
        consumer_control_event: &input::consumer_control::Event,
    ) -> Result<(), Error> {
        self.handle_device_control_consumer_control_event(
            context,
            consumer_control_event,
            event.event_time,
        )?;
        Ok(())
    }

    fn handle_keyboard_event(
        &mut self,
        context: &mut ViewAssistantContext,
        _event: &input::Event,
        keyboard_event: &input::keyboard::Event,
    ) -> Result<(), Error> {
        if self.handle_device_control_keyboard_event(context, keyboard_event)? {
            return Ok(());
        }

        if !self.owns_display {
            return Ok(());
        }

        if self.handle_control_keyboard_event(context, keyboard_event)? {
            return Ok(());
        }

        if let Some((terminal, _)) = self.terminals.get_mut(&self.active_terminal_id) {
            // Get input sequence and write it to the active terminal.
            let app_cursor = terminal.mode();
            if let Some(string) = get_input_sequence_for_key_event(keyboard_event, app_cursor) {
                terminal
                    .write_all(string.as_bytes())
                    .unwrap_or_else(|e| println!("failed to write to terminal: {}", e));

                // Scroll to bottom on input.
                self.scroll_active_terminal(Scroll::Bottom);
            }
        }

        Ok(())
    }

    fn handle_pointer_event(
        &mut self,
        _context: &mut ViewAssistantContext,
        _event: &input::Event,
        pointer_event: &input::pointer::Event,
    ) -> Result<(), Error> {
        match &pointer_event.phase {
            input::pointer::Phase::Down(location) => {
                self.active_pointer_id = Some(pointer_event.pointer_id.clone());
                self.start_pointer_location = location.to_f32();
            }
            input::pointer::Phase::Moved(location) => {
                if Some(pointer_event.pointer_id.clone()) == self.active_pointer_id {
                    let location_offset = location.to_f32() - self.start_pointer_location;

                    fn div_and_trunc(value: f32, divisor: f32) -> isize {
                        (value / divisor).trunc() as isize
                    }

                    // Movement along X-axis changes active terminal.
                    let tab_width = self.tab_width as f32 * self.cell_size.width;
                    let mut terminal_offset = div_and_trunc(location_offset.x, tab_width);
                    while terminal_offset > 0 {
                        self.previous_active_terminal();
                        self.start_pointer_location.x += tab_width;
                        terminal_offset -= 1;
                    }
                    while terminal_offset < 0 {
                        self.next_active_terminal();
                        self.start_pointer_location.x -= tab_width;
                        terminal_offset += 1;
                    }

                    // Movement along Y-axis scrolls active terminal.
                    let cell_offset = div_and_trunc(location_offset.y, self.cell_size.height);
                    if cell_offset != 0 {
                        self.scroll_active_terminal(Scroll::Lines(cell_offset as isize));
                        self.start_pointer_location.y += cell_offset as f32 * self.cell_size.height;
                    }
                }
            }
            input::pointer::Phase::Up => {
                if Some(pointer_event.pointer_id.clone()) == self.active_pointer_id {
                    self.active_pointer_id = None;
                }
            }
            _ => (),
        }
        Ok(())
    }

    fn handle_message(&mut self, message: carnelian::Message) {
        if let Some(message) = message.downcast_ref::<ViewMessages>() {
            match message {
                ViewMessages::AddTerminalMessage(id, terminal, make_active) => {
                    let terminal = terminal.try_clone().expect("failed to clone terminal");
                    let has_output = true;
                    let display_offset = terminal.display_offset();
                    let at_top = display_offset == terminal.history_size();
                    let at_bottom = display_offset == 0;
                    self.terminals
                        .insert(*id, (terminal, TerminalStatus { has_output, at_top, at_bottom }));
                    // Rebuild the scene after a terminal is added. This should
                    // be fine as it is rare that a terminal is added.
                    self.scene_details = None;
                    self.app_sender.request_render(self.view_key);
                    if *make_active {
                        self.set_active_terminal(*id);
                    }
                }
                ViewMessages::RequestTerminalUpdateMessage(id) => {
                    if let Some((terminal, status)) = self.terminals.get_mut(id) {
                        let has_output = if *id == self.active_terminal_id {
                            self.app_sender.request_render(self.view_key);
                            false
                        } else {
                            true
                        };
                        let display_offset = terminal.display_offset();
                        let at_top = display_offset == terminal.history_size();
                        let at_bottom = display_offset == 0;
                        let new_status = TerminalStatus { has_output, at_top, at_bottom };
                        let old_status = mem::replace(status, new_status);
                        if new_status != old_status {
                            self.update_status();
                        }
                    }
                }
            }
        }
    }

    fn ownership_changed(&mut self, owned: bool) -> Result<(), Error> {
        self.owns_display = owned;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_create_view() -> Result<(), Error> {
        let _ = VirtualConsoleViewAssistant::new_for_test()?;
        Ok(())
    }

    #[test]
    fn power_button_press_handler_repetitive_presses() {
        use carnelian::input::consumer_control::Event;

        let mut handler = RepeatedButtonPressHandler::new(ConsumerControlButton::Power);
        const EVENT_DOWN: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Down,
            button: ConsumerControlButton::Power,
        };
        const EVENT_UP: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Up,
            button: ConsumerControlButton::Power,
        };

        handler.handle_button_event(&EVENT_DOWN, 1_000_000_000);
        handler.handle_button_event(&EVENT_UP, 1_100_000_000);
        assert_eq!(handler.press_durations_ns, vec![100_000_000]);
        assert_eq!(handler.down_times_ns, vec![1_000_000_000]);

        handler.handle_button_event(&EVENT_DOWN, 1_200_000_000);
        handler.handle_button_event(&EVENT_UP, 1_400_000_000);
        assert_eq!(handler.press_durations_ns, vec![100_000_000, 200_000_000]);
        assert_eq!(handler.down_times_ns, vec![1_000_000_000, 1_200_000_000]);
    }

    #[test]
    fn power_button_press_handler_press_timeout() {
        use carnelian::input::consumer_control::Event;

        let mut handler = RepeatedButtonPressHandler::new(ConsumerControlButton::Power);
        const EVENT_DOWN: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Down,
            button: ConsumerControlButton::Power,
        };
        const EVENT_UP: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Up,
            button: ConsumerControlButton::Power,
        };

        handler.handle_button_event(&EVENT_DOWN, 1_000_000_000);
        handler.handle_button_event(&EVENT_UP, 1_100_000_000);
        assert_eq!(handler.press_durations_ns, vec![100_000_000]);
        assert_eq!(handler.down_times_ns, vec![1_000_000_000]);

        handler.handle_button_event(&EVENT_DOWN, 5_100_000_000);
        handler.handle_button_event(&EVENT_UP, 5_300_000_000);
        assert_eq!(handler.press_durations_ns, vec![200_000_000]);
        assert_eq!(handler.down_times_ns, vec![5_100_000_000]);
    }

    #[test]
    fn power_button_press_handler_non_power_key_ignored() {
        use carnelian::input::consumer_control::Event;

        let mut handler = RepeatedButtonPressHandler::new(ConsumerControlButton::Power);
        const POWER_EVENT_DOWN: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Down,
            button: ConsumerControlButton::Power,
        };
        const POWER_EVENT_UP: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Up,
            button: ConsumerControlButton::Power,
        };
        const FUNCTION_EVENT_DOWN: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Down,
            button: ConsumerControlButton::Function,
        };
        const FUNCTION_EVENT_UP: Event = Event {
            phase: carnelian::input::consumer_control::Phase::Up,
            button: ConsumerControlButton::Function,
        };

        handler.handle_button_event(&POWER_EVENT_DOWN, 1_000_000_000);
        handler.handle_button_event(&POWER_EVENT_UP, 1_100_000_000);
        assert_eq!(handler.press_durations_ns, vec![100_000_000]);
        assert_eq!(handler.down_times_ns, vec![1_000_000_000]);

        handler.handle_button_event(&FUNCTION_EVENT_DOWN, 1_200_000_000);
        handler.handle_button_event(&FUNCTION_EVENT_UP, 1_300_000_000);
        assert_eq!(handler.press_durations_ns, vec![100_000_000]);
        assert_eq!(handler.down_times_ns, vec![1_000_000_000]);
    }
}
