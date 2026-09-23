// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::button::{Button, ButtonMessages, ButtonOptions, ButtonShape, SceneBuilderButtonExt};
use crate::constants::constants::{
    ADD_NETWORK_BUTTON_COLOR, BACKGROUND_WHITE, ICONS_PATH, ICON_ADD, ICON_ADD_SIZE,
    ICON_ARROW_BACK, ICON_ARROW_FORWARD, ICON_ARROW_SIZE, ICON_LOCK, ICON_LOCK_SIZE,
    ICON_WIFI_FULL_SIGNAL, ICON_WIFI_MID_SIGNAL, ICON_WIFI_NO_SIGNAL, ICON_WIFI_SIZE,
    ICON_WIFI_WEAK_SIGNAL, LEFT_MARGIN_SPACE, TEXT_COLOR, TEXT_FONT_SIZE, THIN_LINE_COLOR,
    TITLE_COLOR, TITLE_SMALL_FONT_SIZE,
};
use anyhow::Error;
use carnelian::drawing::{load_font, FontFace};
use carnelian::render::rive::load_rive;
use carnelian::render::Context as RenderContext;
use carnelian::scene::facets::{
    RiveFacet, TextFacetOptions, TextHorizontalAlignment, TextVerticalAlignment,
};
use carnelian::scene::layout::{
    Alignment, CrossAxisAlignment, Flex, FlexOptions, MainAxisAlignment, MainAxisSize,
};
use carnelian::scene::scene::{Scene, SceneBuilder};
use carnelian::{
    input, make_message, AppSender, Message, MessageTarget, Point, Size, ViewAssistant,
    ViewAssistantContext, ViewKey,
};
use euclid::{size2, Size2D, UnknownUnit};
use fidl_fuchsia_wlan_policy::SecurityType;
use recovery_util::ota::state_machine::Event;
use recovery_util::wlan::NetworkInfo;
use rive_rs::File;
use std::path::PathBuf;

const ADD_WIFI_NETWORK: &str = "  Add Wi-Fi network";
const ARROW_BUTTON_SIZE: Size2D<f32, UnknownUnit> = size2(375.0, 50.0);
const FONT_PATH: &str = "/pkg/data/fonts/Roboto-Regular.ttf";

pub struct SceneDetails {
    pub(crate) scene: Scene,
    buttons: Vec<Button>,
}

pub struct NetworkViewAssistant {
    app_sender: AppSender,
    view_key: ViewKey,
    scene_details: Option<SceneDetails>,
    icon_file: Option<File>,
    font_face: FontFace,
    networks: Vec<NetworkInfo>,
    first_network: i16,
}

impl NetworkViewAssistant {
    pub fn new(
        app_sender: AppSender,
        view_key: ViewKey,
        networks: Vec<NetworkInfo>,
    ) -> Result<NetworkViewAssistant, Error> {
        let icon_file = load_rive(ICONS_PATH).ok();
        let font_face = load_font(PathBuf::from(FONT_PATH)).expect("Font");
        Ok(NetworkViewAssistant {
            app_sender: app_sender,
            scene_details: None,
            view_key,
            icon_file,
            font_face,
            networks,
            first_network: 0,
        })
    }

    fn button_press(&mut self, text: &str) {
        #[cfg(feature = "debug_logging")]
        println!("====== Button pressed {}", text);
        match text {
            "<" => self.show_networks(-3),
            ">" => self.show_networks(3),
            ADD_WIFI_NETWORK => self
                .app_sender
                .queue_message(MessageTarget::View(self.view_key), make_message(Event::AddNetwork)),
            network => {
                let event = if self.is_secure(network) {
                    Event::UserInput(network.to_string())
                } else {
                    Event::UserInputUnsecuredNetwork(network.to_string())
                };
                self.app_sender
                    .queue_message(MessageTarget::View(self.view_key), make_message(event))
            }
        }
    }

    fn is_secure(&self, network: &str) -> bool {
        for network_info in &self.networks {
            if &network_info.ssid == network {
                return network_info.security_type != SecurityType::None;
            }
        }
        eprintln!("Should not happen: Checking security of an unknown network {}", network);
        false
    }

    fn show_networks(&mut self, adjust_by: i16) {
        // Adjust first_network between the start (0) and three before the end
        let length = self.networks.len() as i16;
        self.first_network = num_traits::clamp(self.first_network + adjust_by, 0, length - 3);
        self.scene_details = None;
    }

    fn network_button(
        &self,
        builder: &mut SceneBuilder,
        network: &NetworkInfo,
        width: f32,
    ) -> Button {
        let network_name = network.ssid.as_str();
        builder.start_group(
            "button_row",
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Max,
                MainAxisAlignment::Start,
                CrossAxisAlignment::Center,
            )),
        );
        let wifi_icon_name = match network.rssi {
            -59..=0 => ICON_WIFI_FULL_SIGNAL,
            -67..=-60 => ICON_WIFI_MID_SIGNAL,
            -79..=-68 => ICON_WIFI_WEAK_SIGNAL,
            _ => ICON_WIFI_NO_SIGNAL,
        };
        builder.add_facet(self.get_facet(Some(wifi_icon_name), ICON_WIFI_SIZE));
        builder.space(size2(10.0, 1.0));
        let button = builder.button(
            network_name,
            None,
            ButtonOptions {
                padding: 20.0,
                shape: ButtonShape::Square,
                bg_fg_swapped: true,
                bg_size: Some(size2(width - 175.0, 50.0)),
                ..ButtonOptions::default()
            },
        );
        if network.security_type != SecurityType::None {
            builder.add_facet(self.get_facet(Some(ICON_LOCK), ICON_LOCK_SIZE));
        }
        builder.end_group(); // button_row
        button
    }

    fn add_line_below(&self, builder: &mut SceneBuilder, width: f32) {
        let line_size = size2(width, 1.0);
        builder.space(size2(10.0, 15.0));
        builder.rectangle(line_size, THIN_LINE_COLOR);
        builder.space(size2(10.0, 10.0));
    }

    fn network_scene(&mut self, context: &ViewAssistantContext) -> SceneDetails {
        let target_size = context.size;
        let mut builder = SceneBuilder::new().background_color(BACKGROUND_WHITE);
        let mut buttons = Vec::new();
        builder.group().column().max_size().main_align(MainAxisAlignment::Start).contents(
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
                builder.space(size2(10.0, 50.0));
                builder.text(
                    self.font_face.clone(),
                    "Connect to Wi-Fi to reinstall software",
                    TITLE_SMALL_FONT_SIZE,
                    Point::zero(),
                    TextFacetOptions {
                        horizontal_alignment: TextHorizontalAlignment::Left,
                        vertical_alignment: TextVerticalAlignment::Center,
                        color: TITLE_COLOR,
                        ..TextFacetOptions::default()
                    },
                );
                builder.space(size2(1.0, 20.0));
                builder.start_row("status row", MainAxisAlignment::Start);
                let text = if self.networks.len() == 0 {
                    "Searching for networks"
                } else {
                    "Available networks"
                };
                builder.text(
                    self.font_face.clone(),
                    text,
                    TEXT_FONT_SIZE,
                    Point::zero(),
                    TextFacetOptions {
                        horizontal_alignment: TextHorizontalAlignment::Left,
                        vertical_alignment: TextVerticalAlignment::Center,
                        color: TEXT_COLOR,
                        ..TextFacetOptions::default()
                    },
                );
                builder.end_row(); // status row
                builder.space(size2(1.0, 10.0));
                for i in self.first_network as usize..(self.first_network + 3) as usize {
                    if i < self.networks.len() {
                        builder.space(size2(10.0, 10.0));
                        buttons.push(self.network_button(
                            builder,
                            &self.networks[i],
                            target_size.width,
                        ));
                        self.add_line_below(builder, target_size.width - 2.0 * LEFT_MARGIN_SPACE);
                    }
                }
                if self.networks.len() > 3 {
                    builder.start_row("control row", MainAxisAlignment::Start);
                    buttons.push(builder.button(
                        "<",
                        self.get_facet(Some(ICON_ARROW_BACK), ICON_ARROW_SIZE),
                        ButtonOptions {
                            padding: 0.0,
                            hide_text: true,
                            bg_fg_swapped: true,
                            bg_size: Some(ARROW_BUTTON_SIZE),
                            ..ButtonOptions::default()
                        },
                    ));
                    builder.text(
                        self.font_face.clone(),
                        "More networks",
                        TEXT_FONT_SIZE,
                        Point::zero(),
                        TextFacetOptions {
                            horizontal_alignment: TextHorizontalAlignment::Left,
                            vertical_alignment: TextVerticalAlignment::Center,
                            color: TEXT_COLOR,
                            ..TextFacetOptions::default()
                        },
                    );
                    buttons.push(builder.button(
                        ">",
                        self.get_facet(Some(ICON_ARROW_FORWARD), ICON_ARROW_SIZE),
                        ButtonOptions {
                            padding: 0.0,
                            hide_text: true,
                            bg_fg_swapped: true,
                            bg_size: Some(ARROW_BUTTON_SIZE),
                            text_alignment: Some(Alignment::center_right()),
                            ..ButtonOptions::default()
                        },
                    ));
                    builder.end_row(); // control row
                    self.add_line_below(builder, target_size.width - 2.0 * LEFT_MARGIN_SPACE);
                }
                builder.space(size2(1.0, 20.0));
                builder.start_row("add network row", MainAxisAlignment::Start);
                buttons.push(builder.button(
                    ADD_WIFI_NETWORK,
                    self.get_facet(Some(ICON_ADD), ICON_ADD_SIZE),
                    ButtonOptions {
                        padding: 0.0,
                        shape: ButtonShape::Square,
                        bg_fg_swapped: true,
                        bg_color: ADD_NETWORK_BUTTON_COLOR,
                        text_alignment: Some(Alignment::center_left()),
                        ..ButtonOptions::default()
                    },
                ));
                builder.end_row(); // add network row
                builder.end_group(); // main column
            },
        );
        let mut scene = builder.build();
        scene.layout(target_size);
        for button in &mut buttons {
            button.set_focused(&mut scene, true);
        }
        SceneDetails { scene, buttons }
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
}

pub trait SceneBuilderExtentions {
    fn add_facet(&mut self, facet: Option<RiveFacet>);
    fn start_row(&mut self, label: &str, main_align: MainAxisAlignment);
    fn end_row(&mut self);
}

impl SceneBuilderExtentions for SceneBuilder {
    fn add_facet(&mut self, facet: Option<RiveFacet>) {
        if let Some(facet) = facet {
            self.facet(Box::new(facet));
        }
    }
    fn start_row(&mut self, label: &str, main_align: MainAxisAlignment) {
        self.start_group(
            label,
            Flex::with_options_ptr(FlexOptions::row(
                MainAxisSize::Max,
                main_align,
                CrossAxisAlignment::Center,
            )),
        );
    }
    fn end_row(&mut self) {
        self.end_group();
    }
}

impl ViewAssistant for NetworkViewAssistant {
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
            self.scene_details.take().unwrap_or_else(|| self.network_scene(context));
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
            for button in scene_details.buttons.iter_mut() {
                button.handle_pointer_event(&mut scene_details.scene, context, &pointer_event);
            }
        }
        Ok(())
    }

    fn handle_message(&mut self, message: Message) {
        if let Some(button_message) = message.downcast_ref::<ButtonMessages>() {
            match button_message {
                ButtonMessages::Pressed(_time, button_text) => {
                    self.button_press(button_text);
                }
            }
        }
    }
}
