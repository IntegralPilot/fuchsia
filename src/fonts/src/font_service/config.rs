// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use config::Config;
use std::sync::OnceLock;

/// Gets a reference to the font service's structured config.
///
/// May be called as many times as you need.
///
/// Takes care that structured configuration is actually taken from the
/// process startup handles at most once.
pub(crate) fn as_ref() -> &'static Config {
    // Once-only initialized structured configuration.
    // In tests, we only need the config to be present, and initialized to true.
    static CONFIG: OnceLock<Config> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        if cfg!(test) {
            // Substitute a safe default in tests, to avoid needing to
            // provide structured config in tests only once.
            Config { verbose_logging: true, font_manifest: String::from("") }
        } else {
            // The "take" call may only be made once.
            Config::take_from_startup_handle()
        }
    });
    &config
}
