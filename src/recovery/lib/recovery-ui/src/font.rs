// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use carnelian::drawing::{FontFace, load_font};
use std::path::PathBuf;
use std::sync::LazyLock;

const DEFAULT_FONT_PATH: &str = "/pkg/data/fonts/Roboto-Regular.ttf";

static FONT_FACE: LazyLock<FontFace> = LazyLock::new(|| {
    load_font(PathBuf::from(DEFAULT_FONT_PATH)).expect("failed to open font file")
});

pub fn get_default_font_face() -> &'static FontFace {
    &FONT_FACE
}
