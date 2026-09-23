// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use settings_common::inspect::event::Nameable;

#[derive(PartialEq, Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NightModeInfo {
    pub night_mode_enabled: Option<bool>,
}

impl Nameable for NightModeInfo {
    const NAME: &str = "NightMode";
}
