// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_fuchsia_settings::AccessibilityProxy;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

pub async fn watch<W: std::io::Write>(
    accessibility_proxy: AccessibilityProxy,
    w: &mut W,
) -> Result<()> {
    handle_mixed_result("AccessibilityWatch", command(accessibility_proxy).await, w).await
}

async fn command(proxy: AccessibilityProxy) -> WatchOrSetResult {
    Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{AccessibilityRequest, AccessibilitySettings};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn validate_accessibility_watch() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            AccessibilityRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            AccessibilityRequest::Watch { responder } => {
                let _ = responder.send(&AccessibilitySettings::default());
            }
        });

        let output = utils::assert_watch!(command(proxy));
        assert_eq!(output, format!("{:#?}", AccessibilitySettings::default()));
        Ok(())
    }
}
