// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"

#include <type_traits>

namespace display {

static_assert(!std::is_copy_constructible_v<DisplayEngineEventsInterface>);
static_assert(!std::is_move_constructible_v<DisplayEngineEventsInterface>);

static_assert(!std::is_final_v<DisplayEngineEventsInterface>);
static_assert(std::is_polymorphic_v<DisplayEngineEventsInterface>);

}  // namespace display
