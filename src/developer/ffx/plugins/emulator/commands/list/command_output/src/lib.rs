// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use emulator_instance::EngineState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// This is the item representing the output for a single
/// emulator instance.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct EmuListItem {
    pub name: String,
    pub state: EngineState,
}
