// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use nix::unistd::gethostname;
use regex::Regex;

pub trait GChecker {
    fn is_gcorp_machine(&self) -> bool;
}

pub struct DefaultGChecker;

impl GChecker for DefaultGChecker {
    fn is_gcorp_machine(&self) -> bool {
        static GCORP_REGEX: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new(r"(c.googlers.com|corp.google.com)").unwrap());
        // check if the developer is a Googler
        let hostname_result = gethostname();
        hostname_result
            .as_ref()
            .ok()
            .and_then(|s| s.to_str())
            .map(|h| GCORP_REGEX.is_match(h))
            .unwrap_or(false)
    }
}
