// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::colors::ColorScheme;
use anyhow::Error;
use carnelian::drawing::DisplayRotation;
use std::convert::TryFrom;
use std::str::FromStr;
use virtcon_config::Config;

pub const MIN_FONT_SIZE: f32 = 16.0;
pub const MAX_FONT_SIZE: f32 = 160.0;

#[derive(Debug, Default)]
pub struct VirtualConsoleArgs {
    pub disable: bool,
    pub keep_log_visible: bool,
    pub show_logo: bool,
    pub keyrepeat: bool,
    pub rounded_corners: bool,
    pub color_scheme: ColorScheme,
    pub keymap: String,
    pub display_rotation: DisplayRotation,
    pub font_size: f32,
    pub dpi: Vec<u32>,
    pub scrollback_rows: u32,
    pub buffer_count: usize,
}

impl TryFrom<Config> for VirtualConsoleArgs {
    type Error = Error;

    fn try_from(config: Config) -> Result<Self, Self::Error> {
        let keymap = match config.key_map.as_str() {
            "qwerty" => "US_QWERTY",
            "dvorak" => "US_DVORAK",
            _ => &config.key_map,
        }
        .to_string();
        let font_size = config.font_size.parse::<f32>()?.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);

        Ok(VirtualConsoleArgs {
            disable: config.disable,
            keep_log_visible: config.keep_log_visible,
            show_logo: config.show_logo,
            keyrepeat: config.keyrepeat,
            rounded_corners: config.rounded_corners,
            color_scheme: ColorScheme::from_str(&config.color_scheme)?,
            keymap,
            display_rotation: DisplayRotation::try_from(config.display_rotation)?,
            font_size,
            dpi: config.dpi,
            scrollback_rows: config.scrollback_rows,
            buffer_count: config.buffer_count as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::LIGHT_COLOR_SCHEME;

    fn new_config() -> Config {
        // Should match defaults set in component manifest.
        Config {
            buffer_count: 1,
            color_scheme: "default".into(),
            disable: false,
            display_rotation: 0,
            dpi: vec![],
            font_size: "16.0".into(),
            keep_log_visible: false,
            show_logo: false,
            keyrepeat: false,
            key_map: "qwerty".into(),
            rounded_corners: false,
            scrollback_rows: 1024,
        }
    }

    #[test]
    fn check_disable() -> Result<(), Error> {
        let config = Config { disable: true, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.disable, true);

        let config = Config { disable: false, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.disable, false);

        Ok(())
    }

    #[test]
    fn check_keep_log_visible() -> Result<(), Error> {
        let config = Config { keep_log_visible: true, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keep_log_visible, true);

        let config = Config { keep_log_visible: false, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keep_log_visible, false);

        Ok(())
    }

    #[test]
    fn check_show_logo() -> Result<(), Error> {
        let config = Config { show_logo: true, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.show_logo, true);

        let config = Config { show_logo: false, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.show_logo, false);

        Ok(())
    }

    #[test]
    fn check_keyrepeat() -> Result<(), Error> {
        let config = Config { keyrepeat: true, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keyrepeat, true);

        let config = Config { keyrepeat: false, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keyrepeat, false);

        Ok(())
    }

    #[test]
    fn check_rounded_corners() -> Result<(), Error> {
        let config = Config { rounded_corners: true, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.rounded_corners, true);

        let config = Config { rounded_corners: false, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.rounded_corners, false);

        Ok(())
    }

    #[test]
    fn check_color_scheme() -> Result<(), Error> {
        let config = Config { color_scheme: "light".into(), ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.color_scheme, LIGHT_COLOR_SCHEME);

        let config = Config { ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.color_scheme, ColorScheme::default());

        Ok(())
    }

    #[test]
    fn check_keymap() -> Result<(), Error> {
        let config = Config { key_map: "US_DVORAK".into(), ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keymap, "US_DVORAK");

        let config = Config { key_map: "dvorak".into(), ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keymap, "US_DVORAK");

        let config = Config { ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.keymap, "US_QWERTY");

        Ok(())
    }

    #[test]
    fn check_display_rotation() -> Result<(), Error> {
        let config = Config { display_rotation: 90, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.display_rotation, DisplayRotation::Deg90);

        let config = Config { display_rotation: 0, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.display_rotation, DisplayRotation::Deg0);

        Ok(())
    }

    #[test]
    fn check_font_size() -> Result<(), Error> {
        let config = Config { font_size: "32.0".into(), ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.font_size, 32.0);

        let config = Config { font_size: "1000000.0".into(), ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.font_size, MAX_FONT_SIZE);

        Ok(())
    }

    #[test]
    fn check_dpi() -> Result<(), Error> {
        let config = Config { dpi: vec![160, 320, 480, 640], ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.dpi, vec![160, 320, 480, 640]);

        Ok(())
    }

    #[test]
    fn check_scrollback_rows() -> Result<(), Error> {
        let config = Config { scrollback_rows: 10_000, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.scrollback_rows, 10_000);

        Ok(())
    }

    #[test]
    fn check_buffer_count() -> Result<(), Error> {
        let config = Config { buffer_count: 2, ..new_config() };
        let args = VirtualConsoleArgs::try_from(config)?;
        assert_eq!(args.buffer_count, 2);

        Ok(())
    }
}
