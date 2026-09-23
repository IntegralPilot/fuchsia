// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use driver_diagnostics_log::PublishOptions;
use std::sync::Once;

static LOGGER_ONCE: Once = Once::new();

pub fn init() {
    // The Fuchsia syslog must not be initialized more than once per process. In the case of MLME,
    // that means we can only call it once for both the client and ap modules. Ensure this by using
    // a shared `Once::call_once()`.
    LOGGER_ONCE.call_once(|| {
        // Initialize logging with a tag that can be used to filter for forwarding to console
        // Ignore failures as they might occur due to previously installed logger by the driver
        // host.
        if let Err(e) = driver_diagnostics_log::initialize(
            PublishOptions::default()
                .tags(&["wlan"])
                .enable_metatag(driver_diagnostics_log::Metatag::Target),
        ) {
            eprintln!("Error initializing logging at driver startup: {e}");
        }
    });
}
