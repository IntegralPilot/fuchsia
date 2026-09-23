// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/inplace-vector.h"

#include <type_traits>

namespace display::internal {

template <>
void InplaceVector<int, 10>::StaticChecks() {
  static_assert(std::is_standard_layout_v<ElementOrEmpty>);
}

}  // namespace display::internal
