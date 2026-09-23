// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::AccessibilityTest;
use fidl_fuchsia_settings::{
    AccessibilitySettings, CaptionFontFamily, CaptionFontStyle, CaptionsSettings,
    ColorBlindnessType, EdgeStyle,
};
use fidl_fuchsia_ui_types::ColorRgba;
mod common;

#[fuchsia::test]
async fn test_accessibility_set_all() {
    const CHANGED_COLOR_BLINDNESS_TYPE: ColorBlindnessType = ColorBlindnessType::Tritanomaly;
    const TEST_COLOR: ColorRgba = ColorRgba { red: 238.0, green: 23.0, blue: 128.0, alpha: 255.0 };
    let changed_font_style = CaptionFontStyle {
        family: Some(CaptionFontFamily::Casual),
        color: Some(TEST_COLOR),
        relative_size: Some(1.0),
        char_edge_style: Some(EdgeStyle::Raised),
        ..Default::default()
    };
    let changed_caption_settings = CaptionsSettings {
        for_media: Some(true),
        for_tts: Some(true),
        font_style: Some(changed_font_style),
        window_color: Some(TEST_COLOR),
        background_color: Some(TEST_COLOR),
        ..Default::default()
    };

    let initial_settings = AccessibilitySettings::default();

    let mut expected_settings = AccessibilitySettings::default();
    expected_settings.audio_description = Some(true);
    expected_settings.screen_reader = Some(true);
    expected_settings.color_inversion = Some(true);
    expected_settings.enable_magnification = Some(true);
    expected_settings.color_correction = Some(CHANGED_COLOR_BLINDNESS_TYPE.into());
    expected_settings.captions_settings = Some(changed_caption_settings);

    let instance = AccessibilityTest::create_realm().await.expect("setting up test realm");

    {
        let proxy = AccessibilityTest::connect_to_accessibilitymarker(&instance);

        // Make a watch call.
        let settings = proxy.watch().await.expect("watch completed");
        assert_eq!(settings, initial_settings);

        // Ensure setting interface propagates correctly
        proxy.set(&expected_settings).await.expect("set completed").expect("set successful");
    }

    {
        let proxy = AccessibilityTest::connect_to_accessibilitymarker(&instance);
        // Ensure retrieved value matches set value
        let settings = proxy.watch().await.expect("watch completed");
        assert_eq!(settings, expected_settings);
    }

    let _ = instance.destroy().await;
}

#[fuchsia::test]
async fn test_accessibility_set_captions() {
    let changed_font_style = CaptionFontStyle {
        family: Some(CaptionFontFamily::Casual),
        color: None,
        relative_size: Some(1.0),
        char_edge_style: None,
        ..Default::default()
    };
    let expected_captions_settings = CaptionsSettings {
        for_media: Some(true),
        for_tts: None,
        font_style: Some(changed_font_style),
        window_color: Some(ColorRgba { red: 238.0, green: 23.0, blue: 128.0, alpha: 255.0 }),
        background_color: None,
        ..Default::default()
    };

    let mut expected_settings = AccessibilitySettings::default();
    expected_settings.captions_settings = Some(expected_captions_settings.clone());

    let instance = AccessibilityTest::create_realm().await.expect("setting up test realm");

    {
        let proxy = AccessibilityTest::connect_to_accessibilitymarker(&instance);

        // Set for_media and window_color in the top-level CaptionsSettings.
        let mut first_set = AccessibilitySettings::default();
        first_set.captions_settings = Some(CaptionsSettings {
            for_media: Some(false),
            for_tts: None,
            font_style: None,
            window_color: expected_captions_settings.clone().window_color,
            background_color: None,
            ..Default::default()
        });
        proxy.set(&first_set).await.expect("set completed").expect("set successful");

        // Set FontStyle and overwrite for_media.
        let mut second_set = AccessibilitySettings::default();
        second_set.captions_settings = Some(CaptionsSettings {
            for_media: expected_captions_settings.clone().for_media,
            for_tts: None,
            font_style: expected_captions_settings.clone().font_style,
            window_color: None,
            background_color: None,
            ..Default::default()
        });
        proxy.set(&second_set).await.expect("set completed").expect("set successful");
    }

    {
        let proxy = AccessibilityTest::connect_to_accessibilitymarker(&instance);
        // Ensure retrieved value matches set value
        let settings = proxy.watch().await.expect("watch completed");
        assert_eq!(settings, expected_settings);
    }

    let _ = instance.destroy().await;
}
