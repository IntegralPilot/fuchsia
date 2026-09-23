// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::button::{Button, ButtonMessages, ButtonOptions, ButtonShape, SceneBuilderButtonExt};
use crate::constants::constants::*;
use crate::font::get_default_font_face;
use anyhow::Error;
use carnelian::drawing::FontFace;
use carnelian::render::rive::load_rive;
use carnelian::render::Context as RenderContext;
use carnelian::scene::facets::{
    RiveFacet, TextFacetOptions, TextHorizontalAlignment, TextVerticalAlignment,
};
use carnelian::scene::layout::{
    Alignment, CrossAxisAlignment, Flex, FlexOptions, MainAxisAlignment, MainAxisSize, Stack,
    StackOptions,
};
use carnelian::scene::scene::{Scene, SceneBuilder};
use carnelian::{
    input, make_message, AppSender, Coord, Message, MessageTarget, Point, Size, ViewAssistant,
    ViewAssistantContext, ViewKey,
};
use euclid::{size2, Size2D, UnknownUnit};
use recovery_util::ota::state_machine::Event;
use rive_rs::File;

#[derive(Debug, Clone)]
pub struct ButtonInfo {
    pub text: &'static str,
    pub icon_name: Option<&'static str>,
    pub border: bool,
    pub reversed: bool,
    pub event: Event,
}

impl ButtonInfo {
    pub fn new(
        text: &'static str,
        icon_name: Option<&'static str>,
        border: bool,
        reversed: bool,
        event: Event,
    ) -> Self {
        Self { text, icon_name, border, reversed, event }
    }
}

type Buttons = Vec<Button>;
type ButtonInfos = Vec<ButtonInfo>;

pub struct SceneDetails {
    pub(crate) scene: Scene,
    buttons: Buttons,
}

pub struct GenericSplitViewAssistant {
    app_sender: AppSender,
    view_key: ViewKey,
    split: ScreenSplit,
    text1: Option<String>,
    text2: Option<String>,
    text3: Option<String>,
    button_infos1: Option<ButtonInfos>,
    button_infos2: Option<ButtonInfos>,
    permission_button_infos: Option<ButtonInfos>,
    icon_name: Option<&'static str>,
    icon_size: Option<Size2D<f32, UnknownUnit>>,
    scene_details: Option<SceneDetails>,
    icon_file: Option<File>,
    logo_file: Option<File>,
    qr_file: Option<File>,
}

impl GenericSplitViewAssistant {
    // TODO(b/259793406) Consolidate into a struct, rename and document arguments to this function
    pub fn new(
        app_sender: AppSender,
        view_key: ViewKey,
        split: ScreenSplit,
        text1: Option<String>,
        text2: Option<String>,
        text3: Option<String>,
        button_infos1: Option<ButtonInfos>,
        button_infos2: Option<ButtonInfos>,
        permission_button_infos: Option<ButtonInfos>,
        icon_name: Option<&'static str>,
        icon_size: Option<Size2D<f32, UnknownUnit>>,
    ) -> Result<GenericSplitViewAssistant, Error> {
        #[cfg(feature = "debug_logging")]
        println!("====== New generic view");
        let icon_file = load_rive(ICONS_PATH).ok();
        let logo_file = load_rive(LOGO_PATH).ok();
        let qr_file = load_rive(QR_PATH).ok();

        Ok(GenericSplitViewAssistant {
            app_sender: app_sender,
            view_key,
            split,
            text1,
            text2,
            text3,
            button_infos1,
            button_infos2,
            permission_button_infos,
            icon_name,
            icon_size,
            scene_details: None,
            icon_file,
            logo_file,
            qr_file,
        })
    }

    fn check_if_a_button_in_array_has_been_pressed(
        &self,
        text: &String,
        button_infos: &ButtonInfos,
    ) -> bool {
        for button_info in button_infos {
            // Generate the event that this button press has caused
            if button_info.text == text {
                #[cfg(feature = "debug_logging")]
                println!("====== Generating event: {:?}", button_info.event);
                self.app_sender.queue_message(
                    MessageTarget::View(self.view_key),
                    make_message(button_info.event.clone()),
                );
                return true;
            }
        }
        false
    }

    fn check_if_any_button_has_been_pressed(&self, text: &String) {
        let mut found = false;
        if let Some(button_infos) = self.button_infos1.as_ref() {
            found = self.check_if_a_button_in_array_has_been_pressed(text, button_infos);
        }
        if !found {
            if let Some(button_infos) = self.button_infos2.as_ref() {
                found = self.check_if_a_button_in_array_has_been_pressed(text, button_infos);
            }
        }
        if !found {
            if let Some(button_infos) = self.permission_button_infos.as_ref() {
                self.check_if_a_button_in_array_has_been_pressed(text, button_infos);
            }
        }
    }

    fn generic_split_scene(&mut self, context: &ViewAssistantContext) -> SceneDetails {
        let target_size = context.size;
        let min_dimension = target_size.width.min(target_size.height);
        let _padding = (min_dimension / 20.0).ceil().max(8.0);
        let mut builder = SceneBuilder::new().background_color(BACKGROUND_WHITE);
        let mut buttons: Buttons = Vec::new();
        let split = self.split.as_percent();
        let face = get_default_font_face();
        builder.group().column().max_size().main_align(MainAxisAlignment::SpaceEvenly).contents(
            |builder| {
                builder.start_group(
                    "main screen",
                    Flex::with_options_ptr(FlexOptions::row(
                        MainAxisSize::Max,
                        MainAxisAlignment::SpaceEvenly,
                        CrossAxisAlignment::Start,
                    )),
                );
                builder.start_group(
                    "left column",
                    Flex::with_options_ptr(FlexOptions::column(
                        MainAxisSize::Min,
                        MainAxisAlignment::SpaceEvenly,
                        CrossAxisAlignment::Start,
                    )),
                );
                builder.start_group(
                    "left row for margin space",
                    Flex::with_options_ptr(FlexOptions::row(
                        MainAxisSize::Min,
                        MainAxisAlignment::Start,
                        CrossAxisAlignment::Start,
                    )),
                );
                builder.space(size2(LEFT_MARGIN_SPACE, 10.0));
                builder.start_group(
                    "left column contents",
                    Flex::with_options_ptr(FlexOptions::column(
                        MainAxisSize::Min,
                        MainAxisAlignment::SpaceEvenly,
                        CrossAxisAlignment::Start,
                    )),
                );
                let width1 = target_size.width * split;
                let width2 = target_size.width - width1;
                builder.space(size2(width1, TOP_SPACE));
                if let Some(text) = self.text1.as_ref() {
                    builder.text(
                        face.clone(),
                        text,
                        TITLE_FONT_SIZE,
                        Point::zero(),
                        TextFacetOptions {
                            horizontal_alignment: TextHorizontalAlignment::Left,
                            vertical_alignment: TextVerticalAlignment::Top,
                            color: TITLE_COLOR,
                            ..TextFacetOptions::default()
                        },
                    );
                }

                self.add_privacy_switch(&mut buttons, &face, builder, width1);

                if let Some(button_infos) = self.button_infos1.as_ref() {
                    buttons.append(&mut self.add_buttons(
                        builder,
                        button_infos,
                        LARGE_BUTTON_FONT_SIZE,
                    ));
                }
                builder.space(size2(10.0, AFTER_MAIN_BUTTON_SPACE));
                builder.start_group(
                    "bottom button row",
                    Flex::with_options_ptr(FlexOptions::row(
                        MainAxisSize::Min,
                        MainAxisAlignment::Start,
                        CrossAxisAlignment::Center,
                    )),
                );
                if let Some(text) = self.text3.as_ref() {
                    builder.text(
                        face.clone(),
                        text,
                        TEXT_FONT_SIZE,
                        Point::zero(),
                        TextFacetOptions {
                            horizontal_alignment: TextHorizontalAlignment::Left,
                            vertical_alignment: TextVerticalAlignment::Bottom,
                            color: TEXT_COLOR,
                            ..TextFacetOptions::default()
                        },
                    );
                }
                if let Some(button_infos) = self.button_infos2.as_ref() {
                    builder.space(size2(10.0, 40.0));
                    buttons.append(&mut self.add_buttons(
                        builder,
                        button_infos,
                        SMALL_BUTTON_FONT_SIZE,
                    ));
                }

                builder.end_group(); // bottom button row
                builder.end_group(); // left column contents
                builder.end_group(); // left column space
                builder.end_group(); // left column

                let options =
                    StackOptions { alignment: Alignment::center_left(), ..StackOptions::default() };
                builder.start_group("right screen", Stack::with_options_ptr(options));
                builder.start_group(
                    "right column",
                    Flex::with_options_ptr(FlexOptions::column(
                        MainAxisSize::Min,
                        MainAxisAlignment::SpaceEvenly,
                        CrossAxisAlignment::Center,
                    )),
                );
                if self.split == ScreenSplit::Wide {
                    self.device_panel(builder, face.clone());
                } else if self.split == ScreenSplit::Even {
                    builder.start_group(
                        "right columm row",
                        Flex::with_options_ptr(FlexOptions::row(
                            MainAxisSize::Min,
                            MainAxisAlignment::Center,
                            CrossAxisAlignment::Center,
                        )),
                    );
                    let mut logo_size = size2(296.0, 170.0);
                    if let Some(icon_size) = self.icon_size {
                        logo_size = icon_size;
                    }
                    builder.space(size2((width2 - logo_size.width) / 2.0, 10.0));
                    self.add_facet(builder, self.icon_name, logo_size);
                    builder.end_group(); // right column row
                }
                builder.end_group(); // right column
                builder.rectangle(size2(width2, target_size.height), BACKGROUND_GREY);
                builder.end_group(); // right screen
                builder.end_group(); // main screen
            },
        );
        let mut scene = builder.build();
        for button in &mut buttons {
            button.set_focused(&mut scene, true);
        }
        scene.layout(target_size);
        SceneDetails { scene, buttons }
    }

    fn add_privacy_switch(
        &mut self,
        buttons: &mut Buttons,
        face: &FontFace,
        builder: &mut SceneBuilder,
        width1: Coord,
    ) {
        let ask_permission = self.permission_button_infos.is_some();
        if let Some(text) = self.text2.as_ref() {
            builder.space(size2(width1, AFTER_TITLE_SPACE));
            builder.text(
                face.clone(),
                text,
                TEXT_FONT_SIZE,
                Point::zero(),
                TextFacetOptions {
                    horizontal_alignment: TextHorizontalAlignment::Left,
                    vertical_alignment: TextVerticalAlignment::Top,
                    color: TEXT_COLOR,
                    ..TextFacetOptions::default()
                },
            );
            if ask_permission {
                builder.space(size2(width1, AFTER_TEXT_SPACE / 2.0));
            } else {
                builder.space(size2(width1, AFTER_TEXT_SPACE));
            }
        }
        if ask_permission {
            buttons.push(self.add_report_permission(
                builder,
                self.permission_button_infos.as_ref().unwrap()[0].clone(),
                &face,
            ));
            builder.space(size2(width1, AFTER_TEXT_SPACE / 2.0));
        }
    }

    fn add_buttons(
        &self,
        builder: &mut SceneBuilder,
        button_infos: &ButtonInfos,
        button_font_size: f32,
    ) -> Buttons {
        let mut buttons: Buttons = Vec::new();
        if !button_infos.is_empty() {
            builder.start_group(
                "Button Row",
                Flex::with_options_ptr(FlexOptions::row(
                    MainAxisSize::Min,
                    MainAxisAlignment::SpaceEvenly,
                    CrossAxisAlignment::End,
                )),
            );
            for button_info in button_infos {
                buttons.push(builder.button(
                    button_info.text,
                    self.get_facet(button_info.icon_name, ICON_ADD_SIZE),
                    ButtonOptions {
                        font_size: button_font_size,
                        shape: ButtonShape::Oval,
                        draw_border: button_info.border,
                        bg_fg_swapped: button_info.reversed,
                        ..ButtonOptions::default()
                    },
                ));
                builder.space(size2(BUTTON_SPACE, 10.0));
            }
            builder.end_group(); // Button Row
        }
        buttons
    }

    fn device_panel(&mut self, builder: &mut SceneBuilder, face: FontFace) {
        builder.start_group(
            &("G Panel Row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Min,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Start,
            )),
        );
        builder.space(size2(20.0, 10.0));
        builder.start_group(
            &("G Panel Column"),
            Flex::with_options_ptr(FlexOptions::column(
                MainAxisSize::Min,
                MainAxisAlignment::SpaceEvenly,
                CrossAxisAlignment::Start,
            )),
        );
        builder.space(size2(10.0, 60.0));
        builder.start_group(
            &("QR Code Row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Min,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Center,
            )),
        );
        if let Some(qr_file) = &self.qr_file {
            let logo = RiveFacet::new_from_file(QR_CODE_SIZE, qr_file, Some(QR_CODE))
                .expect("cannot get QR code from QR file");
            builder.facet(Box::new(logo));
        }
        builder.space(size2(16.0, 1.0));
        builder.add_text(&face, "Scan QR code\nfor Help article", G_TEXT_FONT_SIZE);
        builder.end_group(); // QR Code Row
        builder.space(size2(10.0, 208.0));
        let logo_size = size2(G_LOGO_SIZE, G_LOGO_SIZE);
        if let Some(logo_file) = &self.logo_file {
            let logo = RiveFacet::new_from_file(logo_size, logo_file, None)
                .expect("cannot get logo from logo file");
            builder.facet(Box::new(logo));
        }
        builder.space(size2(10.0, 8.0));
        builder.add_text(&face, "Device information", G_TITLE_FONT_SIZE);
        builder.add_text(&face, "(TO BE IMPLEMENTED)", G_TITLE_FONT_SIZE);
        builder.space(size2(10.0, 8.0));
        builder.add_text(&face, "Product: Product Name", G_TEXT_FONT_SIZE);
        builder.add_text(&face, "Serial number: 123-456-7890", G_TEXT_FONT_SIZE);
        builder.add_text(&face, "Build: 00.00000000.00", G_TEXT_FONT_SIZE);
        builder.add_text(&face, "DRAM:: XXXXX", G_TEXT_FONT_SIZE);
        builder.add_text(&face, "UFS: Unknown", G_TEXT_FONT_SIZE);
        builder.add_text(&face, "Package version: XXXXX", G_TEXT_FONT_SIZE);
        builder.end_group(); // G Panel Column
        builder.end_group(); // G Panel Row
    }

    fn add_report_permission(
        &mut self,
        builder: &mut SceneBuilder,
        button_info: ButtonInfo,
        face: &FontFace,
    ) -> Button {
        builder.start_group(
            &("Permissions Row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Min,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Center,
            )),
        );
        builder.add_text(&face, OPTIONAL_REPORT_TEXT, PERMISSION_FONT_SIZE);
        let button_true = button_info.reversed;
        #[cfg(feature = "debug_logging")]
        println!("====== Privacy button set to {}", button_true);
        let icon_name = if button_true { IMAGE_SWITCH_ON } else { IMAGE_SWITCH_OFF };
        let facet = self.get_facet(Some(icon_name), IMAGE_SWITCH_SIZE);
        let button_text = self.permission_button_infos.as_ref().unwrap()[0].text;
        let button = builder.button(
            button_text,
            facet,
            ButtonOptions { hide_text: true, bg_fg_swapped: true, ..ButtonOptions::default() },
        );
        builder.end_group(); // Permissions Row
        button
    }

    fn get_facet(
        &self,
        artboard_name: Option<&'static str>,
        logo_size: Size2D<f32, UnknownUnit>,
    ) -> Option<RiveFacet> {
        if artboard_name.is_none() {
            return None;
        }
        if let Some(icon_file) = &self.icon_file {
            Some(
                RiveFacet::new_from_file(logo_size, icon_file, artboard_name)
                    .expect("facet_from_file"),
            )
        } else {
            None
        }
    }

    fn add_facet(
        &self,
        builder: &mut SceneBuilder,
        artboard_name: Option<&'static str>,
        logo_size: Size2D<f32, UnknownUnit>,
    ) {
        if let Some(facet) = self.get_facet(artboard_name, logo_size) {
            builder.facet(Box::new(facet));
        }
    }
}

pub trait SceneBuilderTextExt {
    fn add_text(&mut self, face: &FontFace, text: &'static str, size: f32);
}

impl SceneBuilderTextExt for SceneBuilder {
    fn add_text(&mut self, face: &FontFace, text: &'static str, size: f32) {
        self.text(
            face.clone(),
            text,
            size,
            Point::zero(),
            TextFacetOptions {
                horizontal_alignment: TextHorizontalAlignment::Left,
                color: TEXT_COLOR,
                max_width: None,
                ..TextFacetOptions::default()
            },
        );
    }
}

impl ViewAssistant for GenericSplitViewAssistant {
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
        let mut scene_details =
            self.scene_details.take().unwrap_or_else(|| self.generic_split_scene(context));

        scene_details.scene.render(render_context, ready_event, context)?;
        self.scene_details = Some(scene_details);
        context.request_render();
        Ok(())
    }

    fn handle_pointer_event(
        &mut self,
        context: &mut ViewAssistantContext,
        _event: &input::Event,
        pointer_event: &input::pointer::Event,
    ) -> Result<(), Error> {
        if let Some(scene_details) = self.scene_details.as_mut() {
            for button in &mut scene_details.buttons {
                button.handle_pointer_event(&mut scene_details.scene, context, &pointer_event);
            }
        }
        context.request_render();
        Ok(())
    }

    fn handle_message(&mut self, message: Message) {
        if let Some(button_message) = message.downcast_ref::<ButtonMessages>() {
            match button_message {
                ButtonMessages::Pressed(_time, button_text) => {
                    #[cfg(feature = "debug_logging")]
                    println!("====== Received button press: {}", button_text);
                    self.check_if_any_button_has_been_pressed(button_text);
                }
            }
        }
    }
}
