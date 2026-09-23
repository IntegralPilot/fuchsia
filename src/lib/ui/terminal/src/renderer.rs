// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::paths::{
    Line, maybe_path_for_char, maybe_path_for_cursor_style, path_for_strikeout, path_for_underline,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorStyle {
    pub shape: CursorShape,
    pub blinking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flags(pub u8);

impl Flags {
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const STRIKEOUT: Self = Self(1 << 3);
    pub const BOLD_ITALIC: Self = Self(1 << 0 | 1 << 1);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn intersects(&self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Flags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}
use carnelian::Size;
use carnelian::color::Color;
use carnelian::drawing::{FontFace, Glyph, TextGrid};
use carnelian::render::{
    BlendMode, Context as RenderContext, Fill, FillRule, Layer, Raster, Style,
};
use carnelian::scene::{LayerGroup, SceneOrder};
use euclid::{Rect, point2};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;
use std::collections::hash_map::Entry;
use std::mem;

// Supported scale factors.
//
// These values are hard-coded in order to ensure that we use a grid size
// that is efficient and aligns with physical pixels.
const SCALE_FACTORS: &[f32] = &[1.0, 1.25, 2.0, 3.0, 4.0];

/// Returns a scale factor given a set of DPI buckets and an actual DPI value.
pub fn get_scale_factor(dpi: &BTreeSet<u32>, actual_dpi: f32) -> f32 {
    let mut scale_factor = 0;
    for value in dpi.iter() {
        if *value as f32 > actual_dpi {
            break;
        }
        scale_factor += 1;
    }
    *SCALE_FACTORS.get(scale_factor).unwrap_or(SCALE_FACTORS.last().unwrap())
}

/// Returns the cell size given a cell height.
pub fn cell_size_from_cell_height(font_set: &FontSet, height: f32) -> Size {
    let rounded_height = height.round();

    // Use a cell width that matches the horizontal advance of character
    // '0' as closely as possible. This minimizes the amount of horizontal
    // stretching used for glyph outlines. Fallback to half of cell height
    // if glyph '0' is missing.
    let face = &font_set.font.face;
    let width = face.glyph_index('0').map_or(height / 2.0, |glyph_index| {
        let ascent = face.ascender() as f32;
        let descent = face.descender() as f32;
        let horizontal_advance =
            face.glyph_hor_advance(glyph_index).expect("glyph_hor_advance") as f32;
        rounded_height * horizontal_advance / (ascent - descent)
    });

    Size::new(width.round(), rounded_height)
}

#[derive(Clone)]
pub struct FontSet {
    font: FontFace,
    bold_font: Option<FontFace>,
    italic_font: Option<FontFace>,
    bold_italic_font: Option<FontFace>,
    fallback_fonts: Vec<FontFace>,
}

impl FontSet {
    pub fn new(
        font: FontFace,
        bold_font: Option<FontFace>,
        italic_font: Option<FontFace>,
        bold_italic_font: Option<FontFace>,
        fallback_fonts: Vec<FontFace>,
    ) -> Self {
        Self { font, bold_font, italic_font, bold_italic_font, fallback_fonts }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum LayerContent {
    Cursor(CursorStyle),
    Char((char, Flags)),
}

#[derive(PartialEq)]
struct LayerId {
    content: LayerContent,
    rgb: Rgb,
}

fn maybe_raster_for_cursor_style(
    render_context: &mut RenderContext,
    cursor_style: CursorStyle,
    cell_size: &Size,
) -> Option<Raster> {
    maybe_path_for_cursor_style(render_context, cursor_style, cell_size).as_ref().map(|p| {
        let mut raster_builder = render_context.raster_builder().expect("raster_builder");
        raster_builder.add(p, None);
        raster_builder.build()
    })
}

fn maybe_fallback_glyph_for_char(
    render_context: &mut RenderContext,
    c: char,
    cell_size: &Size,
) -> Option<Glyph> {
    maybe_path_for_char(render_context, c, cell_size).as_ref().map(|p| {
        let mut raster_builder = render_context.raster_builder().expect("raster_builder");
        raster_builder.add(p, None);
        let raster = raster_builder.build();
        let bounding_box = Rect::from_size(*cell_size);
        Glyph { raster, bounding_box }
    })
}

fn maybe_glyph_for_char(
    context: &mut RenderContext,
    c: char,
    flags: Flags,
    textgrid: &TextGrid,
    font_set: &FontSet,
) -> Option<Glyph> {
    let maybe_bold_italic_font = match flags & Flags::BOLD_ITALIC {
        Flags::BOLD => font_set.bold_font.as_ref(),
        Flags::ITALIC => font_set.italic_font.as_ref(),
        Flags::BOLD_ITALIC => font_set.bold_italic_font.as_ref(),
        _ => None,
    };
    let scale = textgrid.scale;
    let offset = textgrid.offset;

    // Glyph search order:
    //
    // 1. Bold/italic font first if appropriate.
    // 2. Regular font.
    // 3. Fallback fonts.
    //
    // The fallback font can be used to provide icons/emojis
    // that are not expected to be part of the regular font.
    for font in maybe_bold_italic_font
        .iter()
        .map(|font| *font)
        .chain(std::iter::once(&font_set.font))
        .chain(font_set.fallback_fonts.iter())
    {
        if let Some(glyph_index) = font.face.glyph_index(c) {
            let glyph = Glyph::with_scale_and_offset(context, font, scale, offset, glyph_index);
            return Some(glyph);
        }
    }

    // Try fallback glyph if we failed to locate glyph in fonts.
    maybe_fallback_glyph_for_char(context, c, &textgrid.cell_size)
}

fn maybe_raster_for_char(
    context: &mut RenderContext,
    c: char,
    flags: Flags,
    textgrid: &TextGrid,
    font_set: &FontSet,
) -> Option<Raster> {
    // Get a potential glyph for this character.
    let maybe_glyph = maybe_glyph_for_char(context, c, flags, textgrid, font_set);

    // Create an extra raster if underline or strikeout flag is set.
    let maybe_extra_raster = if flags.intersects(Flags::UNDERLINE | Flags::STRIKEOUT) {
        let mut raster_builder = context.raster_builder().expect("raster_builder");
        if flags.contains(Flags::UNDERLINE) {
            // TODO(https://fxbug.dev/42172477): Avoid glyph overlap.
            let line_metrics = font_set.font.face.underline_metrics();
            raster_builder.add(
                &path_for_underline(
                    &textgrid.cell_size,
                    context,
                    line_metrics.map(|line_metrics| Line::new(line_metrics, textgrid)),
                ),
                None,
            );
        }
        if flags.contains(Flags::STRIKEOUT) {
            let line_metrics = font_set.font.face.strikeout_metrics();
            raster_builder.add(
                &path_for_strikeout(
                    &textgrid.cell_size,
                    context,
                    line_metrics.map(|line_metrics| Line::new(line_metrics, textgrid)),
                ),
                None,
            );
        }
        Some(raster_builder.build())
    } else {
        None
    };

    // Return a union of glyph raster and extra raster.
    match (maybe_glyph, maybe_extra_raster) {
        (Some(glyph), Some(extra_raster)) => Some(glyph.raster + extra_raster),
        (Some(glyph), None) => Some(glyph.raster),
        (None, Some(extra_raster)) => Some(extra_raster),
        _ => None,
    }
}

fn maybe_raster_for_layer_content(
    render_context: &mut RenderContext,
    content: &LayerContent,
    column: usize,
    row: usize,
    textgrid: &TextGrid,
    font_set: &FontSet,
    raster_cache: &mut FxHashMap<LayerContent, Option<Raster>>,
) -> Option<Raster> {
    raster_cache
        .entry(*content)
        .or_insert_with(|| match content {
            LayerContent::Cursor(cursor_style) => {
                maybe_raster_for_cursor_style(render_context, *cursor_style, &textgrid.cell_size)
            }
            LayerContent::Char((c, flags)) => {
                maybe_raster_for_char(render_context, *c, *flags, textgrid, font_set)
            }
        })
        .as_ref()
        .map(|r| {
            let cell_size = &textgrid.cell_size;
            let cell_position =
                point2(cell_size.width * column as f32, cell_size.height * row as f32);
            let raster = r.clone().translate(cell_position.to_vector().to_i32());
            // Add empty raster to enable caching of the translated cursor.
            // TODO: add more appropriate API for this.
            let empty_raster = {
                let raster_builder = render_context.raster_builder().unwrap();
                raster_builder.build()
            };
            raster + empty_raster
        })
}

fn make_color(term_color: &Rgb) -> Color {
    Color { r: term_color.r, g: term_color.g, b: term_color.b, a: 0xff }
}

#[derive(PartialEq, Debug)]
pub struct RenderableLayer {
    pub order: usize,
    pub column: usize,
    pub row: usize,
    pub content: LayerContent,
    pub rgb: Rgb,
}

pub struct Offset {
    pub column: usize,
    pub row: usize,
}

pub fn renderable_layers<'b>(
    screen: &'b vt100::Screen,
    default_bg: Rgb,
    default_fg: Rgb,
    offset: &'b Offset,
) -> impl Iterator<Item = RenderableLayer> + 'b {
    let columns = screen.size().1 as usize;
    let rows = screen.size().0 as usize;
    let stride = columns * 4;

    let resolve_color = move |vt100_color: vt100::Color, is_fg: bool| -> Rgb {
        match vt100_color {
            vt100::Color::Default => {
                if is_fg {
                    default_fg
                } else {
                    default_bg
                }
            }
            vt100::Color::Idx(idx) => {
                // simple fallback
                let ansi_colors = [
                    Rgb { r: 0, g: 0, b: 0 },
                    Rgb { r: 170, g: 0, b: 0 },
                    Rgb { r: 0, g: 170, b: 0 },
                    Rgb { r: 170, g: 85, b: 0 },
                    Rgb { r: 0, g: 0, b: 170 },
                    Rgb { r: 170, g: 0, b: 170 },
                    Rgb { r: 0, g: 170, b: 170 },
                    Rgb { r: 170, g: 170, b: 170 },
                    Rgb { r: 85, g: 85, b: 85 },
                    Rgb { r: 255, g: 85, b: 85 },
                    Rgb { r: 85, g: 255, b: 85 },
                    Rgb { r: 255, g: 255, b: 85 },
                    Rgb { r: 85, g: 85, b: 255 },
                    Rgb { r: 255, g: 85, b: 255 },
                    Rgb { r: 85, g: 255, b: 255 },
                    Rgb { r: 255, g: 255, b: 255 },
                ];
                if (idx as usize) < ansi_colors.len() {
                    ansi_colors[idx as usize]
                } else {
                    default_fg
                }
            }
            vt100::Color::Rgb(r, g, b) => Rgb { r, g, b },
        }
    };

    let cursor_pos = screen.cursor_position();
    let hide_cursor = screen.hide_cursor();

    (0..rows).flat_map(move |r| {
        (0..columns).flat_map(move |col| {
            let cell = screen.cell(r as u16, col as u16);
            let row_idx = r + offset.row;
            let cell_order = row_idx * stride + (col + offset.column);
            let is_cursor = !hide_cursor && cursor_pos.0 == r as u16 && cursor_pos.1 == col as u16;

            let (mut fg, mut bg, inverse, bold, italic, underline, contents, has_contents) =
                if let Some(cell) = cell {
                    (
                        cell.fgcolor(),
                        cell.bgcolor(),
                        cell.inverse(),
                        cell.bold(),
                        cell.italic(),
                        cell.underline(),
                        cell.contents().to_string(),
                        cell.has_contents(),
                    )
                } else {
                    (
                        vt100::Color::Default,
                        vt100::Color::Default,
                        false,
                        false,
                        false,
                        false,
                        String::new(),
                        false,
                    )
                };

            if inverse {
                std::mem::swap(&mut fg, &mut bg);
            }

            if is_cursor {
                std::mem::swap(&mut fg, &mut bg);
            }

            let fg_rgb = resolve_color(fg, true);
            let bg_rgb = resolve_color(bg, false);

            let is_default_bg = matches!(bg, vt100::Color::Default) && !is_cursor && !inverse;

            let mut layers = Vec::with_capacity(3);

            if !is_default_bg || is_cursor {
                layers.push(RenderableLayer {
                    order: cell_order,
                    column: col,
                    row: row_idx,
                    content: LayerContent::Cursor(CursorStyle {
                        shape: CursorShape::Block,
                        blinking: false,
                    }),
                    rgb: bg_rgb,
                });
            }

            let mut flags = Flags::empty();
            if bold {
                flags = Flags(flags.0 | Flags::BOLD.0);
            }
            if italic {
                flags = Flags(flags.0 | Flags::ITALIC.0);
            }
            if underline {
                flags = Flags(flags.0 | Flags::UNDERLINE.0);
            }

            let c = if has_contents { contents.chars().next().unwrap_or(' ') } else { ' ' };
            let layer_content = LayerContent::Char((if c == '\t' { ' ' } else { c }, flags));

            layers.push(RenderableLayer {
                order: cell_order + columns * 3,
                column: col,
                row: row_idx,
                content: layer_content,
                rgb: fg_rgb,
            });

            layers.into_iter()
        })
    })
}
pub struct Renderer {
    textgrid: TextGrid,
    raster_cache: FxHashMap<LayerContent, Option<Raster>>,
    layers: FxHashMap<SceneOrder, LayerId>,
    old_layers: FxHashSet<SceneOrder>,
    new_layers: FxHashSet<SceneOrder>,
}

impl Renderer {
    pub fn new(font_set: &FontSet, cell_size: &Size) -> Self {
        let textgrid = TextGrid::new(&font_set.font, cell_size);
        let raster_cache = FxHashMap::default();
        let layers = FxHashMap::default();
        let old_layers = FxHashSet::default();
        let new_layers = FxHashSet::default();

        Self { textgrid, raster_cache, layers, old_layers, new_layers }
    }

    pub fn render<I>(
        &mut self,
        layer_group: &mut dyn LayerGroup,
        render_context: &mut RenderContext,
        font_set: &FontSet,
        layers: I,
    ) where
        I: IntoIterator<Item = RenderableLayer>,
    {
        let raster_cache = &mut self.raster_cache;
        let textgrid = &self.textgrid;

        // Process all layers and update the layer group as needed.
        for RenderableLayer { order, column, row, content, rgb } in layers.into_iter() {
            let id = LayerId { content, rgb };
            let order = SceneOrder::try_from(order).unwrap_or_else(|e| panic!("{}", e));

            // Remove from old layers.
            self.old_layers.remove(&order);

            match self.layers.entry(order) {
                Entry::Occupied(entry) => {
                    if *entry.get() != id {
                        let raster = maybe_raster_for_layer_content(
                            render_context,
                            &id.content,
                            column,
                            row,
                            textgrid,
                            font_set,
                            raster_cache,
                        );
                        if let Some(raster) = raster {
                            let value = entry.into_mut();
                            *value = id;

                            let did_not_exist = self.new_layers.insert(order);
                            assert!(
                                did_not_exist,
                                "multiple layers with order: {}",
                                order.as_u32()
                            );
                            layer_group.insert(
                                order,
                                Layer {
                                    raster,
                                    clip: None,
                                    style: Style {
                                        fill_rule: FillRule::NonZero,
                                        fill: Fill::Solid(make_color(&rgb)),
                                        blend_mode: BlendMode::Over,
                                    },
                                },
                            );
                        } else {
                            entry.remove_entry();
                            layer_group.remove(order);
                        }
                    } else {
                        let did_not_exist = self.new_layers.insert(order);
                        assert!(did_not_exist, "multiple layers with order: {}", order.as_u32());
                    }
                }
                Entry::Vacant(entry) => {
                    let raster = maybe_raster_for_layer_content(
                        render_context,
                        &id.content,
                        column,
                        row,
                        textgrid,
                        font_set,
                        raster_cache,
                    );
                    if let Some(raster) = raster {
                        entry.insert(id);
                        let did_not_exist = self.new_layers.insert(order);
                        assert!(did_not_exist, "multiple layers with order: {}", order.as_u32());
                        layer_group.insert(
                            order,
                            Layer {
                                raster,
                                clip: None,
                                style: Style {
                                    fill_rule: FillRule::NonZero,
                                    fill: Fill::Solid(make_color(&rgb)),
                                    blend_mode: BlendMode::Over,
                                },
                            },
                        );
                    }
                }
            }
        }

        // Remove any remaining old layers.
        for order in self.old_layers.drain() {
            self.layers.remove(&order);
            layer_group.remove(order);
        }

        // Swap old layers for new layers.
        mem::swap(&mut self.old_layers, &mut self.new_layers);
    }
}
