// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use fdomain_fuchsia_settings::{DisplayProxy, DisplaySettings};
use ffx_setui_display_args::SetArgs;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

pub async fn set<W: std::io::Write>(proxy: DisplayProxy, args: SetArgs, w: &mut W) -> Result<()> {
    handle_mixed_result("DisplaySet", command(proxy, DisplaySettings::from(args)).await, w).await
}

async fn command(proxy: DisplayProxy, settings: DisplaySettings) -> WatchOrSetResult {
    if settings == DisplaySettings::default() {
        return Err(format_err!("At least one option is required. Use --help to see options."));
    }

    Ok(Either::Set(if let Err(err) = proxy.set(&settings).await? {
        format!("{:?}", err)
    } else {
        format!("Successfully set Display to {:?}", SetArgs::from(settings))
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{DisplayRequest, LowLightMode, Theme, ThemeMode, ThemeType};
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_set() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DisplayRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            DisplayRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let display = SetArgs {
            brightness: None,
            auto_brightness_level: None,
            auto_brightness: Some(true),
            low_light_mode: None,
            theme: None,
            screen_enabled: None,
        };
        let response = set(proxy, display, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        SetArgs {
            brightness: Some(0.5),
            auto_brightness_level: None,
            auto_brightness: Some(false),
            low_light_mode: None,
            theme: None,
            screen_enabled: None,
        };
        "Test display set() output with non-empty input."
    )]
    #[test_case(
        SetArgs {
            brightness: None,
            auto_brightness_level: Some(0.8),
            auto_brightness: Some(true),
            low_light_mode: Some(LowLightMode::Enable),
            theme: Some(Theme {
                theme_type: Some(ThemeType::Dark),
                theme_mode: Some(ThemeMode::AUTO),
                ..Default::default()
            }),
            screen_enabled: Some(true),
        };
        "Test display set() output with a different non-empty input."
    )]
    #[fuchsia::test]
    async fn validate_display_set_output(expected_display: SetArgs) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DisplayRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            DisplayRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output =
            utils::assert_set!(command(proxy, DisplaySettings::from(expected_display.clone())));
        assert_eq!(output, format!("Successfully set Display to {:?}", expected_display));
        Ok(())
    }
}
