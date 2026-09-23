// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Literal {
    pub kind: LiteralKind,
    pub value: String,
}

#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralKind {
    String,
    Numeric,
    Bool,
}
