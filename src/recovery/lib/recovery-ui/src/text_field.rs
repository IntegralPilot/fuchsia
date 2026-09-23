// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::button::{Button, ButtonOptions, ButtonShape, SceneBuilderButtonExt};
use crate::constants::constants::{
    BORDER_WIDTH, ICONS_PATH, ICON_PASSWORD_INVISIBLE, ICON_PASSWORD_VISIBLE,
    ICON_PASSWORD_VISIBLE_SIZE, MIN_SPACE, TEXT_FIELD_FONT_SIZE, TEXT_FIELD_TITLE_SIZE,
};
use crate::font;
use carnelian::color::Color;
use carnelian::render::rive::load_rive;
use carnelian::scene::facets::{
    FacetId, RiveFacet, SetTextMessage, TextFacetOptions, TextVerticalAlignment,
};
use carnelian::scene::layout::{
    Alignment, CrossAxisAlignment, Flex, FlexOptions, MainAxisAlignment, MainAxisSize, Stack,
    StackOptions,
};
use carnelian::scene::scene::{Scene, SceneBuilder};
use carnelian::{input, Coord, Point, ViewAssistantContext};
use derivative::Derivative;
use euclid::{size2, Size2D, UnknownUnit};
use rive_rs::File;
use std::ops::Add;

#[derive(PartialEq, Clone, Copy)]
pub enum TextVisibility {
    Always,
    Toggleable(bool),
}

impl TextVisibility {
    pub fn toggle(&self) -> TextVisibility {
        if let TextVisibility::Toggleable(boolean) = self {
            TextVisibility::Toggleable(!boolean)
        } else {
            TextVisibility::Always
        }
    }
}

#[derive(Debug, Derivative)]
#[derivative(Default)]
pub struct TextFieldOptions {
    #[derivative(Default(value = "true"))]
    pub draw_border: bool,
    #[derivative(Default(value = "TEXT_FIELD_FONT_SIZE"))]
    pub text_size: f32,
    #[derivative(Default(value = "TEXT_FIELD_TITLE_SIZE"))]
    pub title_size: f32,
    #[derivative(Default(value = "6.0"))]
    pub padding: f32,
    #[derivative(Default(value = "ButtonShape::Oval"))]
    pub shape: ButtonShape,
}

pub struct TextField {
    title: String,
    text: String,
    privacy: TextVisibility,
    text_field: FacetId,
    button: Option<Button>,
    // We need to keep this file open while icons are in use
    _icon_file: Option<File>,
}

impl TextField {
    pub fn new(
        title: String,
        text: String,
        privacy: TextVisibility,
        size: Size2D<f32, UnknownUnit>,
        options: TextFieldOptions,
        builder: &mut SceneBuilder,
    ) -> Self {
        let icon_file = Self::open_icon_file();
        let stack_options =
            StackOptions { alignment: Alignment::top_left(), ..StackOptions::default() };

        builder.start_group("text field", Stack::with_options_ptr(stack_options));
        builder.start_group(
            &("title row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Min,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Start,
            )),
        );
        // TODO(b/259497403): Calculate hardcoded values from screen width
        builder.space(size2(40.0, MIN_SPACE));
        builder.text(
            font::get_default_font_face().clone(),
            &title,
            options.title_size,
            Point::zero(),
            TextFacetOptions {
                background_color: Some(Color::white()),
                ..TextFacetOptions::default()
            },
        );
        builder.end_group(); // title row

        // We need a column here to push the ovals down a little
        // so that the title text cuts the top of the oval.
        builder.start_group(
            "field body column",
            Flex::with_options_ptr(FlexOptions::column(
                MainAxisSize::Min,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Start,
            )),
        );
        builder.space(size2(MIN_SPACE, options.title_size / 2.0));
        let stack_options =
            StackOptions { alignment: Alignment::center_left(), ..StackOptions::default() };
        builder.start_group("field body", Stack::with_options_ptr(stack_options));
        builder.start_group(
            &("Text row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Max,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Center,
            )),
        );
        // TODO(b/259497403): Calculate hardcoded values from screen width
        builder.space(size2(35.0, MIN_SPACE));
        let formatted_text = Self::format_text(text.clone(), privacy);
        let text_field = builder.text(
            font::get_default_font_face().clone(),
            &formatted_text,
            options.text_size,
            Point::zero(),
            TextFacetOptions {
                color: Color::new(),
                vertical_alignment: TextVerticalAlignment::Center,
                ..TextFacetOptions::default()
            },
        );
        builder.end_group(); // Text row

        let button = match privacy {
            TextVisibility::Toggleable(text_visible) => {
                Some(Self::add_privacy_button(&icon_file, &title, text_visible, builder))
            }
            TextVisibility::Always => None,
        };

        // This row is necessary to align the rectangles to create a border
        builder.start_group(
            &("Border row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Max,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Center,
            )),
        );
        builder.space(size2(BORDER_WIDTH / 2.0, BORDER_WIDTH / 2.0));
        let bg_size = size;
        let corner: Coord = Coord::from((bg_size.height) * (options.shape as i32 as f32) / 100.0);
        builder.rounded_rectangle(bg_size, corner, Color::white());
        builder.end_group(); // Border row

        let bg_size = bg_size.add(size2(BORDER_WIDTH, BORDER_WIDTH));
        let corner: Coord = Coord::from((bg_size.height) * (options.shape as i32 as f32) / 100.0);
        builder.rounded_rectangle(bg_size, corner, Color::new());
        builder.end_group(); // field body
        builder.end_group(); // field body column
        builder.end_group(); // text field

        Self { title, text, privacy, text_field, button, _icon_file: icon_file }
    }

    fn add_privacy_button(
        icon_file: &Option<File>,
        title: &String,
        text_visible: bool,
        builder: &mut SceneBuilder,
    ) -> Button {
        builder.start_group(
            &("Icon row"),
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Max,
                MainAxisAlignment::End,
                CrossAxisAlignment::Center,
            )),
        );

        let button = builder.button(
            &title,
            Self::get_eye_icon(icon_file, text_visible),
            ButtonOptions {
                hide_text: true,
                bg_fg_swapped: true,
                shape: ButtonShape::Rounded,
                ..ButtonOptions::default()
            },
        );
        // TODO(b/259497403): Calculate hardcoded values from screen width
        builder.space(size2(140.0, MIN_SPACE));
        builder.end_group(); // Icon row
        button
    }

    fn open_icon_file() -> Option<File> {
        let icon_file = load_rive(ICONS_PATH);
        match icon_file {
            Ok(file) => Some(file),
            Err(error) => {
                eprintln!("Cannot read Rive icon file: {}", error);
                None
            }
        }
    }

    fn get_eye_icon(icon_file: &Option<File>, visible: bool) -> Option<RiveFacet> {
        match icon_file {
            Some(icon_file) => {
                let icon_name =
                    if visible { ICON_PASSWORD_VISIBLE } else { ICON_PASSWORD_INVISIBLE };
                let facet = RiveFacet::new_from_file(
                    ICON_PASSWORD_VISIBLE_SIZE,
                    &icon_file,
                    Some(icon_name),
                );
                match facet {
                    Ok(facet) => Some(facet),
                    Err(error) => {
                        eprintln!("failed to read password icon from file: {}", error);
                        None
                    }
                }
            }
            None => None,
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn get_title(&self) -> &String {
        &self.title
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    pub fn format_text(text: String, privacy: TextVisibility) -> String {
        if TextVisibility::Toggleable(false) == privacy {
            format!("{:*<1$}", "", text.len())
        } else {
            text
        }
    }

    pub fn update_text(&mut self, scene: &mut Scene, text: String) {
        self.set_text(text.clone());
        let formatted_text = Self::format_text(text, self.privacy);
        scene.send_message(&self.text_field, Box::new(SetTextMessage { text: formatted_text }));
    }

    pub fn set_privacy(&mut self, privacy: TextVisibility) {
        self.privacy = privacy;
        self.set_text(self.text.clone());
    }

    pub fn set_focused(&mut self, scene: &mut Scene, focused: bool) {
        if let Some(button) = &mut self.button {
            button.set_focused(scene, focused);
        }
    }

    pub fn handle_pointer_event(
        &mut self,
        scene: &mut Scene,
        context: &mut ViewAssistantContext,
        pointer_event: &input::pointer::Event,
    ) {
        if let Some(button) = &mut self.button {
            button.handle_pointer_event(scene, context, &pointer_event);
        }
    }
}

pub trait SceneBuilderTextFieldExt {
    fn text_field(
        &mut self,
        title: String,
        text: String,
        privacy: TextVisibility,
        size: Size2D<f32, UnknownUnit>,
        options: TextFieldOptions,
    ) -> TextField;
}

impl SceneBuilderTextFieldExt for SceneBuilder {
    fn text_field(
        &mut self,
        title: String,
        text: String,
        privacy: TextVisibility,
        size: Size2D<f32, UnknownUnit>,
        options: TextFieldOptions,
    ) -> TextField {
        TextField::new(title, text, privacy, size, options, self)
    }
}
