// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use settings_common::inspect::event::Nameable;
use settings_common::utils::Merge;

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayInfo {
    /// The last brightness value that was manually set.
    pub manual_brightness_value: f32,
    pub auto_brightness_value: f32,
    pub auto_brightness: bool,
    pub screen_enabled: bool,
    pub low_light_mode: LowLightMode,
    pub theme: Option<Theme>,
}

impl DisplayInfo {
    pub(crate) const fn new(
        auto_brightness: bool,
        manual_brightness_value: f32,
        auto_brightness_value: f32,
        screen_enabled: bool,
        low_light_mode: LowLightMode,
        theme: Option<Theme>,
    ) -> DisplayInfo {
        DisplayInfo {
            manual_brightness_value,
            auto_brightness_value,
            auto_brightness,
            screen_enabled,
            low_light_mode,
            theme,
        }
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.manual_brightness_value.is_finite() && self.auto_brightness_value.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_info_is_finite() {
        let mut info = DisplayInfo::new(false, 0.5, 0.5, true, LowLightMode::Disable, None);
        assert!(info.is_finite());

        info.manual_brightness_value = f32::NAN;
        assert!(!info.is_finite());

        info.manual_brightness_value = 0.5;
        info.auto_brightness_value = f32::NAN;
        assert!(!info.is_finite());

        info.manual_brightness_value = f32::INFINITY;
        assert!(!info.is_finite());

        info.auto_brightness_value = f32::NEG_INFINITY;
        assert!(!info.is_finite());
    }
}

impl Nameable for DisplayInfo {
    const NAME: &str = "Display";
}

#[derive(Debug, Default, PartialEq, Copy, Clone)]
pub struct SetDisplayInfo {
    pub manual_brightness_value: Option<f32>,
    pub auto_brightness_value: Option<f32>,
    pub auto_brightness: Option<bool>,
    pub screen_enabled: Option<bool>,
    pub low_light_mode: Option<LowLightMode>,
    pub theme: Option<Theme>,
}

impl Merge<SetDisplayInfo> for DisplayInfo {
    fn merge(&self, other: SetDisplayInfo) -> Self {
        Self {
            manual_brightness_value: other
                .manual_brightness_value
                .unwrap_or(self.manual_brightness_value),
            auto_brightness_value: other
                .auto_brightness_value
                .unwrap_or(self.auto_brightness_value),
            auto_brightness: other.auto_brightness.unwrap_or(self.auto_brightness),
            screen_enabled: other.screen_enabled.unwrap_or(self.screen_enabled),
            low_light_mode: other.low_light_mode.unwrap_or(self.low_light_mode),
            theme: other.theme.or(self.theme),
        }
    }
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize, Hash, Eq)]
pub enum LowLightMode {
    /// Device should not be in low-light mode.
    Disable,
    /// Device should not be in low-light mode and should transition
    /// out of it immediately.
    DisableImmediately,
    /// Device should be in low-light mode.
    Enable,
}

#[derive(PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ThemeType {
    Unknown,
    Default,
    Light,
    Dark,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ThemeMode: u32 {
        /// Product can choose a theme based on ambient cues.
        const AUTO = 0b00000001;
    }
}

bitflags_serde_legacy::impl_traits!(ThemeMode);

#[derive(Debug, Clone, PartialEq, Copy, Serialize, Deserialize)]
pub struct Theme {
    pub theme_type: Option<ThemeType>,
    pub theme_mode: ThemeMode,
}

impl Theme {
    pub(super) fn new(theme_type: Option<ThemeType>, theme_mode: ThemeMode) -> Self {
        Self { theme_type, theme_mode }
    }
}

/// Builder for `Theme` that with a `build` method that returns
/// an `Option` that will be None if all the fields of the Theme would
/// otherwise be empty.
pub struct ThemeBuilder {
    theme_type: Option<ThemeType>,
    theme_mode: ThemeMode,
}

impl ThemeBuilder {
    pub(super) fn new() -> Self {
        Self { theme_type: None, theme_mode: ThemeMode::empty() }
    }

    pub(super) fn set_theme_type(&mut self, theme_type: Option<ThemeType>) -> &mut Self {
        self.theme_type = theme_type;
        self
    }

    pub(super) fn set_theme_mode(&mut self, theme_mode: ThemeMode) -> &mut Self {
        self.theme_mode = theme_mode;
        self
    }

    pub(super) fn build(&self) -> Option<Theme> {
        if self.theme_type.is_none() && self.theme_mode.is_empty() {
            None
        } else {
            Some(Theme { theme_type: self.theme_type, theme_mode: self.theme_mode })
        }
    }
}
