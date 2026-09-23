// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// To properly bookend the logs even when we have an early disconnect, we have this struct which
/// auto-logs on drop.
pub struct FfxLogGuard<'a> {
    log_id: &'a Option<String>,
}

impl<'a> FfxLogGuard<'a> {
    pub fn new(log_id: &'a Option<String>) -> Self {
        if let Some(log_id) = log_id {
            log::debug!("====> Starting ffx session: {}", log_id);
        }
        Self { log_id }
    }
}

impl Drop for FfxLogGuard<'_> {
    fn drop(&mut self) {
        if let Some(log_id) = self.log_id {
            log::debug!("====> Ending ffx session: {}", log_id);
        }
    }
}
