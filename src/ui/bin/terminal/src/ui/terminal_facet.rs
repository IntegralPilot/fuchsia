// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use carnelian::color::Color;
use carnelian::drawing::path_for_rounded_rectangle;
use carnelian::render::{
    BlendMode, Context as RenderContext, Fill, FillRule, Layer, Raster, Style,
};
use carnelian::scene::facets::Facet;
use carnelian::scene::{LayerGroup, SceneOrder};
use carnelian::{AppSender, Coord, Rect, Size, ViewAssistantContext, ViewKey};
use fuchsia_trace as ftrace;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use terminal::{FontSet, Offset, Renderer, TerminalCallbacks, renderable_layers};

pub enum TerminalMessages {
    SetScrollThumbMessage(Option<(Rect, f32)>),
    /// Scroll thumb fade-out starting time and duration.
    SetScrollThumbFadeOutMessage(Option<(Instant, Duration)>),
}

fn raster_for_rounded_rectangle(
    bounds: &Rect,
    corner_radius: Coord,
    render_context: &mut RenderContext,
) -> Raster {
    let mut raster_builder = render_context.raster_builder().expect("raster_builder");
    raster_builder.add(&path_for_rounded_rectangle(bounds, corner_radius, render_context), None);
    raster_builder.build()
}

// Computes the current scroll thumb alpha value for a fade-out effect started for `time_elapsed` with a duration of `total_duration`.
fn scroll_thumb_fade_out_alpha(total_duration: f32, time_elapsed: f32, alpha: f32) -> f32 {
    alpha - (alpha / total_duration) * time_elapsed
}

/// Facet that implements a terminal text grid with a scroll bar.
pub struct TerminalFacet {
    app_sender: Option<AppSender>,
    view_key: ViewKey,
    font_set: FontSet,
    size: Size,
    term: Rc<RefCell<vt100::Parser<TerminalCallbacks>>>,
    scroll_thumb: Option<(Rect, f32)>,
    scroll_thumb_fade_out: Option<(Instant, Duration)>,
    thumb_order: Option<SceneOrder>,
    renderer: Renderer,
}

impl TerminalFacet {
    pub fn new(
        app_sender: Option<AppSender>,
        view_key: ViewKey,
        font_set: FontSet,
        cell_size: &Size,
        term: Rc<RefCell<vt100::Parser<TerminalCallbacks>>>,
    ) -> Self {
        let renderer = Renderer::new(&font_set, cell_size);

        TerminalFacet {
            app_sender,
            view_key,
            font_set,
            size: Size::zero(),
            term,
            scroll_thumb: None,
            scroll_thumb_fade_out: None,
            thumb_order: None,
            renderer,
        }
    }
}

impl Facet for TerminalFacet {
    fn update_layers(
        &mut self,
        _: Size,
        layer_group: &mut dyn LayerGroup,
        render_context: &mut RenderContext,
        view_context: &ViewAssistantContext,
    ) -> Result<(), Error> {
        ftrace::duration!("terminal", "TerminalFacet:update_layers");

        self.size = view_context.size;

        let term = self.term.borrow();
        let cols = term.screen().size().1 as usize;
        let rows = term.screen().size().0 as usize;
        let stride = cols * 4;
        let new_thumb_order =
            SceneOrder::try_from(stride * rows).unwrap_or_else(|e| panic!("{}", e));

        // Remove old scrollbar thumb.
        if let Some(thumb_order) = self.thumb_order.take() {
            layer_group.remove(thumb_order);
        }

        let offset = Offset { column: 0, row: 0 };
        let layers = renderable_layers(
            term.screen(),
            terminal::renderer::Rgb { r: 0, g: 0, b: 0 },
            terminal::renderer::Rgb { r: 255, g: 255, b: 255 },
            &offset,
        );
        self.renderer.render(layer_group, render_context, &self.font_set, layers);

        // Add new scrollbar thumb.
        if let Some((thumb, alpha)) = self.scroll_thumb {
            let alpha =
                self.scroll_thumb_fade_out.map_or(alpha, |(starting_time, total_duration)| {
                    let time_elapsed = starting_time.elapsed().min(total_duration).as_secs_f32();
                    scroll_thumb_fade_out_alpha(total_duration.as_secs_f32(), time_elapsed, alpha)
                });
            // Linear to sRGB.
            let srgb = (alpha.powf(1.0 / 2.2) * 255.0) as u8;
            let color = Color { r: srgb, g: srgb, b: srgb, a: (alpha * 255.0) as u8 };
            let layer = Layer {
                raster: raster_for_rounded_rectangle(&thumb, 2.0, render_context),
                clip: None,
                style: Style {
                    fill_rule: FillRule::NonZero,
                    fill: Fill::Solid(color),
                    blend_mode: BlendMode::Over,
                },
            };
            layer_group.insert(new_thumb_order, layer);
            self.thumb_order = Some(new_thumb_order);

            if self.scroll_thumb_fade_out.is_some() {
                if let Some(app_sender) = &self.app_sender {
                    app_sender.request_render(self.view_key);
                }
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, message: Box<dyn Any>) {
        if let Some(message) = message.downcast_ref::<TerminalMessages>() {
            match message {
                TerminalMessages::SetScrollThumbMessage(thumb) => {
                    if thumb.is_none() {
                        self.scroll_thumb_fade_out = None;
                    }
                    self.scroll_thumb = *thumb;
                }
                TerminalMessages::SetScrollThumbFadeOutMessage(thumb_fade_out) => {
                    self.scroll_thumb_fade_out = *thumb_fade_out;
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

    #[test]
    fn scroll_thumb_fade_out() {
        let alpha: f32 = 1.0;
        let fade_out_duration = 1.0;

        assert_eq!(scroll_thumb_fade_out_alpha(fade_out_duration, 0.25, alpha), 0.75);
        assert_eq!(scroll_thumb_fade_out_alpha(fade_out_duration, 0.50, alpha), 0.5);
        assert_eq!(scroll_thumb_fade_out_alpha(fade_out_duration, 0.75, alpha), 0.25);
        assert_eq!(scroll_thumb_fade_out_alpha(fade_out_duration, 1.0, alpha), 0.0);
    }
}
