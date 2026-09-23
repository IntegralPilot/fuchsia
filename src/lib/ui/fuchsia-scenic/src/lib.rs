// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod flatland;
mod sysmem;
mod view_ref_pair;
mod view_token_pair;
pub use self::sysmem::*;
pub use self::view_ref_pair::*;
pub use self::view_token_pair::*;

use fidl_fuchsia_ui_display_color as _;

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum DisplayRotation {
    None = 0,
    By90Degrees = 90,
    By180Degrees = 180,
    By270Degrees = 270,
}
