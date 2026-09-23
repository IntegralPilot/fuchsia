// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use carnelian::app::Config;
use carnelian::color::Color;
use carnelian::drawing::path_for_rectangle;
use carnelian::render::{BlendMode, Context as RenderContext, Fill, FillRule, Layer, Path, Style};
use carnelian::scene::facets::Facet;
use carnelian::scene::scene::{Scene, SceneBuilder, SceneOrder};
use carnelian::scene::LayerGroup;
use carnelian::{
    App, AppAssistant, AppAssistantPtr, AppSender, AssistantCreatorFunc, Coord, LocalBoxFuture,
    Rect, Size, ViewAssistant, ViewAssistantContext, ViewAssistantPtr, ViewKey,
};
use euclid::{point2, size2, vec2, Transform2D};

struct StaticSquareAppAssistant;

impl AppAssistant for StaticSquareAppAssistant {
    fn setup(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn create_view_assistant(&mut self, _: ViewKey) -> Result<ViewAssistantPtr, Error> {
        StaticSquareViewAssistant::new()
    }

    fn filter_config(&mut self, config: &mut Config) {
        config.display_resource_release_delay = std::time::Duration::new(0, 0);
    }
}

struct SceneDetails {
    scene: Scene,
}

struct StaticSquareFacet {
    square_color: Color,
    square_path: Option<Path>,
    size: Size,
}

impl StaticSquareFacet {
    fn new(square_color: Color, size: Size) -> Self {
        Self { square_color, square_path: None, size }
    }

    fn clone_square_path(&self) -> Path {
        self.square_path.as_ref().expect("square_path").clone()
    }
}

impl Facet for StaticSquareFacet {
    fn update_layers(
        &mut self,
        size: Size,
        layer_group: &mut dyn LayerGroup,
        render_context: &mut RenderContext,
        _view_context: &ViewAssistantContext,
    ) -> Result<(), Error> {
        const SQUARE_PATH_SIZE: Coord = 1.0;
        const SQUARE_PATH_SIZE_2: Coord = SQUARE_PATH_SIZE / 2.0;

        let center_x = size.width * 0.5;
        let center_y = size.height * 0.5;
        self.size = size;
        let square_size = size.width.min(size.height) * 0.5;

        if self.square_path.is_none() {
            let top_left = point2(-SQUARE_PATH_SIZE_2, -SQUARE_PATH_SIZE_2);
            let square = Rect::new(top_left, size2(SQUARE_PATH_SIZE, SQUARE_PATH_SIZE));
            let square_path = path_for_rectangle(&square, render_context);
            self.square_path.replace(square_path);
        }

        let transformation =
            Transform2D::scale(square_size, square_size).then_translate(vec2(center_x, center_y));
        let mut raster_builder = render_context.raster_builder().expect("raster_builder");
        raster_builder.add(&self.clone_square_path(), Some(&transformation));
        let square_raster = raster_builder.build();

        layer_group.insert(
            SceneOrder::default(),
            Layer {
                raster: square_raster,
                clip: None,
                style: Style {
                    fill_rule: FillRule::NonZero,
                    fill: Fill::Solid(self.square_color),
                    blend_mode: BlendMode::Over,
                },
            },
        );
        Ok(())
    }

    fn calculate_size(&self, _available: Size) -> Size {
        self.size
    }
}

struct StaticSquareViewAssistant {
    background_color: Color,
    square_color: Color,
    scene_details: Option<SceneDetails>,
}

impl StaticSquareViewAssistant {
    fn new() -> Result<ViewAssistantPtr, Error> {
        let square_color = Color { r: 0xff, g: 0x00, b: 0xff, a: 0xff };
        let background_color = Color { r: 0x00, g: 0x00, b: 0xff, a: 0xff };

        Ok(Box::new(StaticSquareViewAssistant {
            background_color,
            square_color,
            scene_details: None,
        }))
    }

    fn ensure_scene_built(&mut self, size: Size) {
        if self.scene_details.is_none() {
            let mut builder =
                SceneBuilder::new().background_color(self.background_color).animated(true);
            #[allow(clippy::collection_is_never_read)]
            let mut square = None;
            builder.group().stack().center().contents(|builder| {
                let square_facet = StaticSquareFacet::new(self.square_color, size);
                square = Some(builder.facet(Box::new(square_facet)));
            });
            let scene = builder.build();
            self.scene_details = Some(SceneDetails { scene });
        }
    }
}

impl ViewAssistant for StaticSquareViewAssistant {
    fn resize(&mut self, new_size: &Size) -> Result<(), Error> {
        self.scene_details = None;
        self.ensure_scene_built(*new_size);
        Ok(())
    }

    fn get_scene(&mut self, size: Size) -> Option<&mut Scene> {
        self.ensure_scene_built(size);
        Some(&mut self.scene_details.as_mut().unwrap().scene)
    }
}

fn make_app_assistant_fut(
    _app_sender: &AppSender,
) -> LocalBoxFuture<'_, Result<AppAssistantPtr, Error>> {
    let f = async move {
        let assistant = Box::new(StaticSquareAppAssistant);
        Ok::<AppAssistantPtr, Error>(assistant)
    };
    Box::pin(f)
}

fn make_app_assistant() -> AssistantCreatorFunc {
    Box::new(make_app_assistant_fut)
}

fn main() -> Result<(), Error> {
    App::run(make_app_assistant())
}
