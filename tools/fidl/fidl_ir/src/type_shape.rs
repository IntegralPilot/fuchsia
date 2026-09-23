// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct TypeShape {
    pub alignment: u32,
    pub depth: u32,
    pub has_flexible_envelope: bool,
    pub has_padding: bool,
    pub inline_size: u32,
    pub max_handles: u32,
    pub max_out_of_line: u32,
}
