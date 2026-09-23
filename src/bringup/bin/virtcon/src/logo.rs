// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::log::LogClient;
use anyhow::Error;

const LOGO_TEXT: &str = "
\r
                                      ff    ff  ff\r
                                   ff  fffffffffff ff\r
                                  f ffffffffffffffff f\r
                                ff ffffffffffffffffffff\r
                                f fffffffff        ffff\r
                               f ffffffff            ff\r
                               f fffffff              f\r
                              ff fffffff\r
                              f  ffffff\r
                               fffffffff             f\r
                        fffffff                    fff\r
                    ffffffffffffffffffffffff   ffffff\r
                 ffffffffffffffffffffffffffffffffff\r
                ffffff   fffffff         ffffff\r
               fffff  fff      ffffffffff\r
              fffff ff         fffffff  f\r
              ffff fff         fffffff ff\r
             fffff ff         ffffffff f\r
              ffff fff       ffffffff  f\r
              ffff ffffffffffffffffff f\r
               ffff fffffffffffffff  f\r
                fffff  ffffffffff  ff\r
                  fffff         fff\r
                    fffffffffffff\r
";

pub struct Logo;
impl Logo {
    pub fn start<T: LogClient>(client: &T, id: u32) -> Result<(), Error>
where {
        let client = client.clone();
        let terminal =
            client.create_terminal(id, "logo".to_string()).expect("failed to create terminal");
        let term = terminal.clone_term();

        let mut parser = term.borrow_mut();
        parser.process(LOGO_TEXT.as_bytes());
        client.request_update(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::ColorScheme;
    use crate::terminal::Terminal;
    use fuchsia_async as fasync;

    #[derive(Default, Clone)]
    struct TestLogClient;

    impl LogClient for TestLogClient {
        fn create_terminal(&self, _id: u32, title: String) -> Result<Terminal, Error> {
            Ok(Terminal::new(title, ColorScheme::default(), 1024, None))
        }
        fn request_update(&self, _id: u32) {}
    }

    #[fasync::run_singlethreaded(test)]
    async fn can_start_logo() -> Result<(), Error> {
        let client = TestLogClient::default();
        let _ = Logo::start(&client, 0)?;
        Ok(())
    }
}

#[cfg(feature = "boot_framebuffer_demo")]
mod graphic {
    use anyhow::Error;
    use carnelian::color::Color;
    use carnelian::render::rive::{RenderCache, load_rive};
    use carnelian::render::{Context, Fill};
    use carnelian::scene::LayerGroup;
    use carnelian::scene::facets::Facet;
    use carnelian::scene::scene::{SceneBuilder, SceneOrder};
    use carnelian::{Point, Size, ViewAssistantContext};

    struct LogoFacet {
        file: rive_rs::File,
        cache: RenderCache,
        size: Size,
    }

    impl Facet for LogoFacet {
        fn update_layers(
            &mut self,
            _size: Size,
            layers: &mut dyn LayerGroup,
            context: &mut Context,
            _view: &ViewAssistantContext,
        ) -> Result<(), Error> {
            let artboard =
                self.file.artboard().ok_or_else(|| anyhow::anyhow!("missing logo artboard"))?;
            let artboard = artboard.as_ref();
            artboard.advance(0.0);
            self.cache.with_renderer(context, |renderer| {
                artboard.draw(
                    renderer,
                    rive_rs::layout::align(
                        rive_rs::layout::Fit::Contain,
                        rive_rs::layout::Alignment::center(),
                        rive_rs::math::Aabb::new(0.0, 0.0, self.size.width, self.size.height),
                        artboard.bounds(),
                    ),
                );
            });
            layers.clear();
            for (i, mut layer) in self.cache.layers.drain(..).enumerate() {
                if let Fill::Solid(color) = &mut layer.style.fill {
                    *color = Color { r: 237, g: 29, b: 127, a: color.a };
                }
                layers.insert(SceneOrder::try_from(i)?, layer);
            }
            Ok(())
        }

        fn calculate_size(&self, _available: Size) -> Size {
            self.size
        }
    }

    pub fn add_logo(builder: &mut SceneBuilder, screen: Size) -> Result<(), Error> {
        let side = 320.0_f32.min(screen.width / 3.0).min(screen.height / 3.0);
        let logo = LogoFacet {
            file: load_rive("/pkg/data/fuchsia-logo.riv")?,
            cache: RenderCache::new(),
            size: Size::new(side, side),
        };
        builder.facet_at_location(
            Box::new(logo),
            Point::new((screen.width - side) / 2.0, (screen.height - side) / 2.0),
        );
        Ok(())
    }
}

#[cfg(feature = "boot_framebuffer_demo")]
pub use graphic::add_logo;
