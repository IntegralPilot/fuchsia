// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const TEST_DEP_CONST: i32 = test_lib::TEST_CONST;

#[path = "test_dep_priv_mod.rs"]
mod test_dep_priv_mod;
pub use test_dep_priv_mod::*;
