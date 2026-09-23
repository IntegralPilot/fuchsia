// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use fdomain_fuchsia_settings::{AccessibilityProxy, AccessibilitySettings};
use ffx_setui_accessibility_args::SetArgs;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

pub async fn set<W: std::io::Write>(
    accessibility_proxy: AccessibilityProxy,
    args: SetArgs,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result("AccessibilitySet", command(accessibility_proxy, args).await, writer).await
}

async fn command(proxy: AccessibilityProxy, options: SetArgs) -> WatchOrSetResult {
    let mut settings = AccessibilitySettings::default();
    settings.audio_description = options.audio_description;
    settings.screen_reader = options.screen_reader;
    settings.color_inversion = options.color_inversion;
    settings.enable_magnification = options.enable_magnification;
    settings.color_correction = options.color_correction;

    if settings == AccessibilitySettings::default() {
        return Err(format_err!("At least one option is required. Use --help to see options."));
    }

    Ok(Either::Set(if let Err(err) = proxy.set(&settings).await? {
        format!("{:?}", err)
    } else {
        format!("Successfully set AccessibilitySettings")
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{AccessibilityRequest, ColorBlindnessType};
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_set() {
        const TRUE: bool = true;
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AccessibilityRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            AccessibilityRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let args = SetArgs {
            audio_description: Some(TRUE),
            screen_reader: None,
            color_inversion: None,
            enable_magnification: None,
            color_correction: None,
        };
        let response = set(proxy, args, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        SetArgs {
            audio_description: Some(true),
            screen_reader: Some(false),
            color_inversion: Some(false),
            enable_magnification: None,
            color_correction: Some(ColorBlindnessType::Protanomaly),
        };
        "Test set other settings."
    )]
    #[test_case(
        SetArgs {
            audio_description: Some(false),
            screen_reader: Some(true),
            color_inversion: None,
            enable_magnification: Some(false),
            color_correction: Some(ColorBlindnessType::Deuteranomaly),
        };
        "Test set other settings with different inputs."
    )]
    #[fuchsia::test]
    async fn validate_accessibility_set(expected_set: SetArgs) -> Result<()> {
        let set_clone = expected_set.clone();
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AccessibilityRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            AccessibilityRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, set_clone));
        assert_eq!(output, format!("Successfully set AccessibilitySettings"));
        Ok(())
    }
}
