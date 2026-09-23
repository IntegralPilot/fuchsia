// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/engine-info.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<EngineInfo>);
static_assert(std::is_trivially_assignable_v<EngineInfo, EngineInfo>);
static_assert(std::is_trivially_copyable_v<EngineInfo>);
static_assert(std::is_trivially_copy_constructible_v<EngineInfo>);
static_assert(std::is_trivially_destructible_v<EngineInfo>);
static_assert(std::is_trivially_move_assignable_v<EngineInfo>);
static_assert(std::is_trivially_move_constructible_v<EngineInfo>);

// Ensure that the `EngineInfo` constants match the FIDL constants.
static_assert(fuchsia_hardware_display_engine::wire::kMaxAllowedMaxLayerCount ==
              EngineInfo::kMaxAllowedMaxLayerCount);
static_assert(fuchsia_hardware_display_engine::wire::kMaxAllowedMaxConnectedDisplayCount ==
              EngineInfo::kMaxAllowedMaxConnectedDisplayCount);

}  // namespace display
