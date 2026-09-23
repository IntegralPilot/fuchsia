// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/rectangle_f.h"

#include "src/ui/scenic/lib/types/rectangle.h"

namespace types {

// static
constexpr bool RectangleF::IsValid(const Rectangle& rectangle, bool should_assert) {
  const ConstructorArgs args{.x = static_cast<float>(rectangle.x()),
                             .y = static_cast<float>(rectangle.y()),
                             .width = static_cast<float>(rectangle.width()),
                             .height = static_cast<float>(rectangle.height())};
  return IsValid(args, should_assert);
}

// static
RectangleF RectangleF::From(const Rectangle& rectangle) {
  return RectangleF({.x = static_cast<float>(rectangle.x()),
                     .y = static_cast<float>(rectangle.y()),
                     .width = static_cast<float>(rectangle.width()),
                     .height = static_cast<float>(rectangle.height())});
}

}  // namespace types
