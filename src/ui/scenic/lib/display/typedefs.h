// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_TYPEDEFS_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_TYPEDEFS_H_

#include "src/ui/scenic/lib/types/blend_mode.h"
#include "src/ui/scenic/lib/types/display_mode.h"
#include "src/ui/scenic/lib/types/extent2.h"
#include "src/ui/scenic/lib/types/rectangle.h"
#include "src/ui/scenic/lib/types/rotate_flip.h"

namespace display {

// These types are fully substitutable.  In other words, there are no additional constraints on the
// valid usage of, say, a `types::BlendMode` vs. a `display::BlendMode`.
using types::BlendMode;
using types::DisplayMode;
using types::Extent2;
using types::Rectangle;
using types::RotateFlip;

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_TYPEDEFS_H_
