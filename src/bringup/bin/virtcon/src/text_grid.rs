// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::colors::ColorScheme;
use carnelian::color::Color;
use carnelian::render::Context as RenderContext;
use carnelian::scene::LayerGroup;
use carnelian::scene::facets::Facet;
use carnelian::{Size, ViewAssistantContext};
use fuchsia_trace::duration;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use terminal::renderer::{CursorShape, CursorStyle, Flags, Rgb};
use terminal::{
    FontSet, LayerContent, Offset, RenderableLayer, Renderer, TerminalCallbacks, renderable_layers,
};
use vt100::Parser;

fn make_rgb(color: &Color) -> Rgb {
    Rgb { r: color.r, g: color.g, b: color.b }
}

/// Facet that implements a virtcon-style text grid with a status bar
/// and terminal output.
pub struct TextGridFacet {
    font_set: FontSet,
    color_scheme: ColorScheme,
    size: Size,
    term: Option<Rc<RefCell<Parser<TerminalCallbacks>>>>,
    status: Vec<(String, Rgb)>,
    status_tab_width: usize,
    renderer: Renderer,
}

pub enum TextGridMessages {
    SetTermMessage(Rc<RefCell<vt100::Parser<TerminalCallbacks>>>),
    ChangeStatusMessage(Vec<(String, Rgb)>),
}

const STATUS_BG: Rgb = Rgb { r: 0, g: 0, b: 0 };

impl TextGridFacet {
    pub fn new(
        font_set: FontSet,
        cell_size: &Size,
        color_scheme: ColorScheme,
        term: Option<Rc<RefCell<Parser<TerminalCallbacks>>>>,
        status: Vec<(String, Rgb)>,
        status_tab_width: usize,
    ) -> Self {
        let renderer = Renderer::new(&font_set, cell_size);

        Self {
            font_set,
            color_scheme,
            size: Size::zero(),
            term,
            status,
            status_tab_width,
            renderer,
        }
    }
}

impl Facet for TextGridFacet {
    fn update_layers(
        &mut self,
        _: Size,
        layer_group: &mut dyn LayerGroup,
        render_context: &mut RenderContext,
        view_context: &ViewAssistantContext,
    ) -> std::result::Result<(), anyhow::Error> {
        duration!("gfx", "TextGrid::update_layers");

        self.size = view_context.size;

        let term = self.term.as_ref().map(|t| t.borrow());
        let status_tab_width = self.status_tab_width;
        let columns = term.as_ref().map(|t| t.screen().size().1 as usize).unwrap_or(1);
        let bg = make_rgb(&self.color_scheme.back);
        // First row is used for the status bar.
        let term_offset = Offset { column: 0, row: 1 };

        // Create an iterator over cells used for the status bar followed by active
        // terminal cells. Each layer has an order, contents, and color.
        //
        // The order of layers will be stable unless the number of columns change.
        //
        // Status bar is a set of background layers, followed by foreground layers.
        let layers = if STATUS_BG != bg {
            Some((0..columns).into_iter().map(|x| RenderableLayer {
                order: x,
                column: x,
                row: 0,
                content: LayerContent::Cursor(CursorStyle {
                    shape: CursorShape::Block,
                    blinking: false,
                }),
                rgb: STATUS_BG,
            }))
        } else {
            None
        }
        .into_iter()
        .flat_map(|iter| iter)
        .chain(self.status.iter().enumerate().flat_map(|(i, (s, rgb))| {
            let start = i * status_tab_width;
            let order = columns + start;
            s.chars().enumerate().map(move |(x, c)| RenderableLayer {
                order: order + x,
                column: start + x,
                row: 0,
                content: LayerContent::Char((c, Flags::empty())),
                rgb: *rgb,
            })
        }))
        .chain(term.iter().flat_map(|term| {
            renderable_layers(
                term.screen(),
                make_rgb(&self.color_scheme.back),
                make_rgb(&self.color_scheme.front),
                &term_offset,
            )
        }));

        self.renderer.render(layer_group, render_context, &self.font_set, layers);

        Ok(())
    }

    fn handle_message(&mut self, message: Box<dyn Any>) {
        if let Some(message) = message.downcast_ref::<TextGridMessages>() {
            match message {
                TextGridMessages::SetTermMessage(term) => {
                    self.term = Some(Rc::clone(term));
                }
                TextGridMessages::ChangeStatusMessage(status) => {
                    self.status = status.clone();
                }
            }
        }
    }

    fn calculate_size(&self, _: Size) -> Size {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use carnelian::drawing::load_font;
    use std::path::PathBuf;

    const FONT: &'static str = "/pkg/data/font.ttf";

    #[test]
    fn can_create_text_grid() -> Result<(), Error> {
        let font = load_font(PathBuf::from(FONT))?;
        let font_set = FontSet::new(font, None, None, None, vec![]);
        let _ = TextGridFacet::new(
            font_set,
            &Size::new(8.0, 16.0),
            ColorScheme::default(),
            None,
            vec![],
            24,
        );
        Ok(())
    }
}
