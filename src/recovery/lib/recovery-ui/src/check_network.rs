// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::button::{Button, ButtonMessages, ButtonOptions, ButtonShape, SceneBuilderButtonExt};
use crate::constants::constants::{
    BACKGROUND_WHITE, BUTTON_SPACE, LEFT_MARGIN_SPACE, TEXT_COLOR, TEXT_FONT_SIZE, TITLE_COLOR,
    TITLE_FONT_SIZE,
};
use crate::generic_view::ButtonInfo;
use crate::text_field::{SceneBuilderTextFieldExt, TextField, TextFieldOptions, TextVisibility};
use anyhow::Error;
use carnelian::drawing::load_font;
use carnelian::render::Context as RenderContext;
use carnelian::scene::facets::{TextFacetOptions, TextHorizontalAlignment, TextVerticalAlignment};
use carnelian::scene::layout::{
    CrossAxisAlignment, Flex, FlexOptions, MainAxisAlignment, MainAxisSize,
};
use carnelian::scene::scene::{Scene, SceneBuilder};
use carnelian::{
    input, make_message, AppSender, Message, MessageTarget, Point, Size, ViewAssistant,
    ViewAssistantContext, ViewKey,
};
use euclid::size2;
use std::path::PathBuf;

const TEXT_FIELD_WIDTH: f32 = 895.0;

pub struct SceneDetails {
    pub(crate) scene: Scene,
    password_field: TextField,
    buttons: Vec<Button>,
}

/// Asks the user whether they want to try again, enter a new network or cancel
pub struct CheckNetworkViewAssistant {
    app_sender: AppSender,
    view_key: ViewKey,
    title_text: String,
    body_text: String,
    network: String,
    password: String,
    privacy: bool,
    button_infos: Vec<ButtonInfo>,
    scene_details: Option<SceneDetails>,
}

impl CheckNetworkViewAssistant {
    pub fn new(
        app_sender: AppSender,
        view_key: ViewKey,
        title_text: String,
        body_text: String,
        network: String,
        password: String,
        button_infos: Vec<ButtonInfo>,
    ) -> Result<CheckNetworkViewAssistant, Error> {
        Ok(CheckNetworkViewAssistant {
            app_sender: app_sender,
            view_key,
            scene_details: None,
            title_text,
            body_text,
            network,
            password,
            privacy: true,
            button_infos,
        })
    }

    fn button_press(&mut self, text: &String) {
        for button_info in &self.button_infos {
            // Generate the event that this button press has caused
            if button_info.text == text {
                #[cfg(feature = "debug_logging")]
                println!("====== Generating event: {:?}", button_info.event);
                self.app_sender.queue_message(
                    MessageTarget::View(self.view_key),
                    make_message(button_info.event.clone()),
                )
            }
        }
        if let Some(scene_details) = &mut self.scene_details {
            if scene_details.password_field.get_title() == text {
                self.privacy = !self.privacy;
                self.scene_details = None;
            }
        }
    }

    pub fn check_network_scene(&mut self, context: &ViewAssistantContext) -> SceneDetails {
        let target_size = context.size;
        let mut builder = SceneBuilder::new().background_color(BACKGROUND_WHITE);
        let mut buttons: Vec<Button> = Vec::new();
        let face = load_font(PathBuf::from("/pkg/data/fonts/Roboto-Regular.ttf")).expect("Font");
        let mut password_field: Option<TextField> = None;
        builder.group().column().max_size().main_align(MainAxisAlignment::SpaceEvenly).contents(
            |builder| {
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
                    "main column",
                    Flex::with_options_ptr(FlexOptions::column(
                        MainAxisSize::Min,
                        MainAxisAlignment::Start,
                        CrossAxisAlignment::Start,
                    )),
                );
                builder.space(size2(10.0, 30.0));
                builder.text(
                    face.clone(),
                    &self.title_text,
                    TITLE_FONT_SIZE,
                    Point::zero(),
                    TextFacetOptions {
                        horizontal_alignment: TextHorizontalAlignment::Left,
                        vertical_alignment: TextVerticalAlignment::Bottom,
                        color: TITLE_COLOR,
                        ..TextFacetOptions::default()
                    },
                );
                builder.space(size2(10.0, 30.0));
                builder.text(
                    face.clone(),
                    &self.body_text,
                    TEXT_FONT_SIZE,
                    Point::zero(),
                    TextFacetOptions {
                        horizontal_alignment: TextHorizontalAlignment::Left,
                        vertical_alignment: TextVerticalAlignment::Bottom,
                        color: TEXT_COLOR,
                        ..TextFacetOptions::default()
                    },
                );
                builder.space(size2(1.0, 50.0));
                builder.text_field(
                    "Network".to_string(),
                    self.network.clone(),
                    TextVisibility::Always,
                    size2(TEXT_FIELD_WIDTH, TEXT_FONT_SIZE * 3.0),
                    TextFieldOptions { text_size: TEXT_FONT_SIZE, ..TextFieldOptions::default() },
                );
                builder.space(size2(10.0, 40.0));
                password_field = Some(builder.text_field(
                    "Password".to_string(),
                    self.password.clone(),
                    TextVisibility::Toggleable(!self.privacy),
                    size2(TEXT_FIELD_WIDTH, TEXT_FONT_SIZE * 3.0),
                    TextFieldOptions { text_size: TEXT_FONT_SIZE, ..TextFieldOptions::default() },
                ));
                builder.space(size2(10.0, 50.0));
                buttons.append(&mut self.add_buttons(builder, &self.button_infos));

                builder.end_group(); // main column
                builder.end_group(); //gui left row for margin space
            },
        );
        let mut scene = builder.build();
        for button in &mut buttons {
            button.set_focused(&mut scene, true);
        }
        password_field.as_mut().unwrap().set_focused(&mut scene, true);
        scene.layout(target_size);
        SceneDetails { scene, password_field: password_field.unwrap(), buttons }
    }

    fn add_buttons(
        &self,
        builder: &mut SceneBuilder,
        button_infos: &Vec<ButtonInfo>,
    ) -> Vec<Button> {
        let mut buttons: Vec<Button> = Vec::new();
        if !button_infos.is_empty() {
            builder.start_group(
                "Button Row",
                Flex::with_options_ptr(FlexOptions::row(
                    MainAxisSize::Max,
                    MainAxisAlignment::End,
                    CrossAxisAlignment::End,
                )),
            );
            for button_info in button_infos {
                buttons.push(builder.button(
                    button_info.text,
                    None,
                    ButtonOptions {
                        shape: ButtonShape::Oval,
                        bg_fg_swapped: button_info.reversed,
                        ..ButtonOptions::default()
                    },
                ));
                builder.space(size2(BUTTON_SPACE, 10.0));
            }
            // Move the buttons in from the right
            builder.space(size2(117.0, 10.0));
            builder.end_group(); // Button Row
        }
        buttons
    }
}

impl ViewAssistant for CheckNetworkViewAssistant {
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
            self.scene_details.take().unwrap_or_else(|| self.check_network_scene(context));
        scene_details.scene.render(render_context, ready_event, context)?;
        self.scene_details = Some(scene_details);
        context.request_render();
        Ok(())
    }

    fn handle_message(&mut self, message: Message) {
        if let Some(button_message) = message.downcast_ref::<ButtonMessages>() {
            match button_message {
                ButtonMessages::Pressed(_time, button_text) => {
                    #[cfg(feature = "debug_logging")]
                    println!("====== Received button press: {}", button_text);
                    self.button_press(button_text);
                }
            }
        }
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
            // We need to pass this to the password field because it is a sub-view
            scene_details.password_field.handle_pointer_event(
                &mut scene_details.scene,
                context,
                &pointer_event,
            );
        }
        Ok(())
    }
}
