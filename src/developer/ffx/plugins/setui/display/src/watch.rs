// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_fuchsia_settings::DisplayProxy;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

pub async fn watch<W: std::io::Write>(proxy: DisplayProxy, w: &mut W) -> Result<()> {
    handle_mixed_result("DisplayWatch", command(proxy).await, w).await
}

async fn command(proxy: DisplayProxy) -> WatchOrSetResult {
    Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{DisplayRequest, DisplaySettings};
    use ffx_setui_display_args::SetArgs;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[test_case(
        SetArgs {
            brightness: None,
            auto_brightness_level: None,
            auto_brightness: None,
            low_light_mode: None,
            theme: None,
            screen_enabled: None,
        };
        "Test display watch() output with empty input."
    )]
    #[test_case(
        SetArgs {
            brightness: Some(0.5),
            auto_brightness_level: None,
            auto_brightness: Some(false),
            low_light_mode: None,
            theme: None,
            screen_enabled: None,
        };
        "Test display watch() output with non-empty input."
    )]
    #[fuchsia::test]
    async fn validate_display_watch_output(expected_display: SetArgs) -> Result<()> {
        let expected_display_clone = expected_display.clone();
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DisplayRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            DisplayRequest::Watch { responder } => {
                let _ = responder.send(&DisplaySettings::from(expected_display.clone()));
            }
        });

        let output = utils::assert_watch!(command(proxy));
        assert_eq!(output, format!("{:#?}", DisplaySettings::from(expected_display_clone)));
        Ok(())
    }
}
